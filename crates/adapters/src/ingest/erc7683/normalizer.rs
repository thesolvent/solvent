//! Decodes an ERC-7683 gasless order into the canonical `Intent`. The envelope is the standard's;
//! the `SolventOrder` inside it is fixed-price, so its amounts map onto `AmountCurve::scalar`.

use alloy::primitives::{Address, U256};
use alloy::sol_types::SolValue;

use solvent_core::deps::ingest::{NormalizeError, Normalizer};
use solvent_core::primitives::ingest::{
    AmountCurve, Exclusivity, Intent, IntentInput, IntentOutput, ProtocolId, RawOrder,
};
use solvent_core::primitives::IntentId;

use super::codec::{order_id, solvent_order_type, GaslessCrossChainOrder, SolventOrder};

pub struct Erc7683Normalizer;

impl Normalizer for Erc7683Normalizer {
    fn normalize(&self, raw: &RawOrder) -> Result<Intent, NormalizeError> {
        let bad = || NormalizeError::Decode(ProtocolId::Erc7683);
        let order = GaslessCrossChainOrder::abi_decode(&raw.payload).map_err(|_| bad())?;
        if order.orderDataType != solvent_order_type() {
            return Err(bad());
        }
        if order.originChainId != U256::from(raw.chain.0) {
            return Err(bad());
        }
        let data = SolventOrder::abi_decode(&order.orderData).map_err(|_| bad())?;

        let exclusivity = (data.exclusiveFiller != Address::ZERO)
            .then(|| Exclusivity::new(data.exclusiveFiller, u64::from(data.exclusivityEnds)));

        Ok(Intent::new(
            IntentId(order_id(&order)),
            ProtocolId::Erc7683,
            IntentInput::new(data.inputToken, AmountCurve::scalar(data.inputAmount)),
            vec![IntentOutput::new(
                data.outputToken,
                AmountCurve::scalar(data.outputAmount),
                data.recipient,
            )],
            u64::from(order.fillDeadline),
            exclusivity,
            order.originSettler,
            raw.chain,
            raw.payload.clone(),
            raw.signature.clone(),
            raw.observed_at,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, keccak256, Bytes, B256};
    use solvent_core::primitives::ChainId;

    fn filler() -> Address {
        address!("5555555555555555555555555555555555555555")
    }

    fn inner(exclusive_filler: Address) -> SolventOrder {
        SolventOrder {
            inputToken: address!("6666666666666666666666666666666666666666"),
            inputAmount: U256::from(500u64),
            outputToken: address!("7777777777777777777777777777777777777777"),
            outputAmount: U256::from(1000u64),
            recipient: address!("8888888888888888888888888888888888888888"),
            exclusiveFiller: exclusive_filler,
            exclusivityEnds: 1000,
        }
    }

    fn envelope(inner: &SolventOrder, order_data_type: B256, chain: u64) -> GaslessCrossChainOrder {
        GaslessCrossChainOrder {
            originSettler: address!("2222222222222222222222222222222222222222"),
            user: address!("3333333333333333333333333333333333333333"),
            nonce: U256::from(42u64),
            originChainId: U256::from(chain),
            openDeadline: 1500,
            fillDeadline: 2000,
            orderDataType: order_data_type,
            orderData: Bytes::from(inner.abi_encode()),
        }
    }

    fn raw_of(order: &GaslessCrossChainOrder, feed_chain: u64) -> RawOrder {
        RawOrder::new(
            ProtocolId::Erc7683,
            ChainId(feed_chain),
            Bytes::from(order.abi_encode()),
            Bytes::from(vec![0xABu8; 65]),
            1234,
        )
    }

    #[test]
    fn maps_a_well_formed_order() {
        let order = envelope(&inner(filler()), solvent_order_type(), 1);
        let raw = raw_of(&order, 1);

        let intent = Erc7683Normalizer.normalize(&raw).expect("well-formed");

        assert_eq!(intent.id, IntentId(order_id(&order)));
        assert_eq!(intent.protocol, ProtocolId::Erc7683);
        assert_eq!(
            intent.input.token,
            address!("6666666666666666666666666666666666666666")
        );
        assert_eq!(intent.input.curve, AmountCurve::scalar(U256::from(500u64)));
        assert_eq!(intent.outputs.len(), 1);
        assert_eq!(
            intent.outputs[0].curve,
            AmountCurve::scalar(U256::from(1000u64))
        );
        assert_eq!(
            intent.outputs[0].recipient,
            address!("8888888888888888888888888888888888888888")
        );
        // The fill deadline is the one a filler must beat; `openDeadline` bounds `openFor` instead.
        assert_eq!(intent.deadline, 2000);
        assert_eq!(intent.exclusivity, Some(Exclusivity::new(filler(), 1000)));
        assert_eq!(
            intent.settler,
            address!("2222222222222222222222222222222222222222")
        );
        assert_eq!(intent.origin_chain, ChainId(1));
        assert_eq!(intent.raw, raw.payload);
        assert_eq!(intent.signature, raw.signature);
        assert_eq!(intent.observed_at, 1234);
    }

    #[test]
    fn open_order_has_no_exclusivity() {
        let order = envelope(&inner(Address::ZERO), solvent_order_type(), 1);
        let intent = Erc7683Normalizer
            .normalize(&raw_of(&order, 1))
            .expect("well-formed");
        assert_eq!(intent.exclusivity, None);
    }

    // A 7683 settler may serve many order types; one we cannot decode is not ours to fill.
    #[test]
    fn rejects_an_unrecognized_order_data_type() {
        let order = envelope(&inner(filler()), keccak256(b"SomeOtherOrder(uint256 x)"), 1);
        assert!(matches!(
            Erc7683Normalizer.normalize(&raw_of(&order, 1)),
            Err(NormalizeError::Decode(ProtocolId::Erc7683))
        ));
    }

    // The envelope names its own origin chain; a feed delivering it under a different one is
    // handing us an order that settles somewhere we are not.
    #[test]
    fn rejects_an_origin_chain_mismatch() {
        let order = envelope(&inner(filler()), solvent_order_type(), 10);
        assert!(matches!(
            Erc7683Normalizer.normalize(&raw_of(&order, 1)),
            Err(NormalizeError::Decode(ProtocolId::Erc7683))
        ));
    }

    #[test]
    fn rejects_a_malformed_payload() {
        let raw = RawOrder::new(
            ProtocolId::Erc7683,
            ChainId(1),
            Bytes::from_static(&[1, 2, 3]),
            Bytes::new(),
            0,
        );
        assert!(matches!(
            Erc7683Normalizer.normalize(&raw),
            Err(NormalizeError::Decode(ProtocolId::Erc7683))
        ));
    }

    // `orderData` that is not a `SolventOrder` despite claiming to be — a settler bug or a spoofed
    // feed; it must not reach routing.
    #[test]
    fn rejects_undecodable_order_data() {
        let mut order = envelope(&inner(filler()), solvent_order_type(), 1);
        order.orderData = Bytes::from_static(&[9, 9, 9]);
        assert!(matches!(
            Erc7683Normalizer.normalize(&raw_of(&order, 1)),
            Err(NormalizeError::Decode(ProtocolId::Erc7683))
        ));
    }
}
