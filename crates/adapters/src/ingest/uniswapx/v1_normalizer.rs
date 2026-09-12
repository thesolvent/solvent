//! Decodes a UniswapX `ExclusiveDutchOrder` (the Orders API's `Limit` order type, on the original
//! `ExclusiveDutchOrderReactor` deployment) into the canonical `Intent`.
//!
//! Simpler than V2: exclusivity is a plain field the swapper signed, not a cosigner override, so
//! there is no separate pre-override/post-override amount to reconcile — `decayStartTime`/
//! `decayEndTime` and every amount are exactly what the swapper signed.

use alloy::primitives::Address;
use alloy::sol_types::SolValue;

use solvent_core::deps::ingest::{NormalizeError, Normalizer};
use solvent_core::primitives::ingest::{
    AmountCurve, Exclusivity, Intent, IntentInput, IntentOutput, IntentParts, ProtocolId, RawOrder,
};
use solvent_core::primitives::IntentId;

use super::codec::{v1_order_hash, V1DutchOrder};

/// Decodes and validates `Limit`-type orders against the deployment this resolver fills for.
pub struct UniswapXV1Normalizer {
    reactor: Address,
}

impl UniswapXV1Normalizer {
    pub fn new(reactor: Address) -> UniswapXV1Normalizer {
        UniswapXV1Normalizer { reactor }
    }
}

impl Normalizer for UniswapXV1Normalizer {
    fn normalize(&self, raw: &RawOrder) -> Result<Intent, NormalizeError> {
        let bad = || NormalizeError::Decode(ProtocolId::UniswapXV1);
        let order = V1DutchOrder::abi_decode(&raw.payload).map_err(|_| bad())?;
        if order.info.reactor != self.reactor {
            return Err(bad());
        }
        // Same reasoning as the V2 normalizer: an arbitrary contract the reactor calls inside our
        // fill can burn unbounded gas or revert on state an `eth_call` cannot reproduce.
        if order.info.additionalValidationContract != Address::ZERO {
            return Err(bad());
        }
        if order.info.deadline < order.decayEndTime {
            return Err(bad());
        }
        // `DutchDecayLib.decay` special-cases `startAmount == endAmount` before it ever looks at
        // the window (`if (startAmount == endAmount) return startAmount;`), so a flat leg never
        // reverts on `EndTimeBeforeStartTime` even with a zero-width or backwards window — real
        // `Limit`-type orders are exactly this: flat, with `decayStartTime == decayEndTime`. Only
        // a leg that actually decays needs a valid window to resolve.
        let window_invalid = order.decayEndTime <= order.decayStartTime;
        let input_decays = order.input.endAmount != order.input.startAmount;
        if window_invalid && input_decays {
            return Err(bad());
        }
        if order.input.endAmount < order.input.startAmount {
            return Err(bad());
        }
        if window_invalid && order.outputs.iter().any(|o| o.endAmount != o.startAmount) {
            return Err(bad());
        }
        if order.outputs.iter().any(|o| o.endAmount > o.startAmount) {
            return Err(bad());
        }

        let start_time = u64::try_from(order.decayStartTime).map_err(|_| bad())?;
        let end_time = u64::try_from(order.decayEndTime).map_err(|_| bad())?;

        let input = IntentInput::new(
            order.input.token,
            AmountCurve::dutch(
                order.input.startAmount,
                order.input.endAmount,
                start_time,
                end_time,
            ),
        );
        let outputs = order
            .outputs
            .iter()
            .map(|o| {
                IntentOutput::new(
                    o.token,
                    AmountCurve::dutch(o.startAmount, o.endAmount, start_time, end_time),
                    o.recipient,
                )
            })
            .collect();

        // Same as V2: the window ends when the decay begins, and the reactor passes
        // `decayStartTime` as its exclusivity end — there is no separate field.
        let exclusivity = (order.exclusiveFiller != Address::ZERO)
            .then(|| {
                u16::try_from(order.exclusivityOverrideBps)
                    .map(|bps| Exclusivity::new(order.exclusiveFiller, start_time, bps))
            })
            .transpose()
            .map_err(|_| bad())?;

        Ok(Intent::new(IntentParts {
            deadline: u64::try_from(order.info.deadline).map_err(|_| bad())?,
            exclusivity,
            settler: order.info.reactor,
            raw: raw.payload.clone(),
            signature: raw.signature.clone(),
            observed_at: raw.observed_at,
            source: raw.source,
            ..IntentParts::new(
                IntentId(v1_order_hash(&order)),
                ProtocolId::UniswapXV1,
                order.info.swapper,
                input,
                outputs,
                raw.chain,
            )
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, Bytes, B256, U256};
    use alloy::signers::local::PrivateKeySigner;
    use alloy::sol_types::SolValue;
    use solvent_core::primitives::ingest::OrderSource;
    use solvent_core::primitives::ChainId;

    use super::super::codec::{DutchInput, DutchOutput, OrderInfo};

    const REACTOR: Address = address!("6000da47483062a0d734ba3dc7576ce6a0b645c4");

    fn key(byte: u8) -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::from([byte; 32])).expect("valid test key")
    }

    fn order(swapper: Address, exclusive_filler: Address, override_bps: u64) -> V1DutchOrder {
        V1DutchOrder {
            info: OrderInfo {
                reactor: REACTOR,
                swapper,
                nonce: U256::from(1u64),
                deadline: U256::from(5000u64),
                additionalValidationContract: Address::ZERO,
                additionalValidationData: Bytes::new(),
            },
            decayStartTime: U256::from(1000u64),
            decayEndTime: U256::from(1100u64),
            exclusiveFiller: exclusive_filler,
            exclusivityOverrideBps: U256::from(override_bps),
            input: DutchInput {
                token: address!("6666666666666666666666666666666666666666"),
                startAmount: U256::from(500u64),
                endAmount: U256::from(500u64),
            },
            outputs: vec![DutchOutput {
                token: address!("7777777777777777777777777777777777777777"),
                startAmount: U256::from(1000u64),
                endAmount: U256::from(900u64),
                recipient: address!("8888888888888888888888888888888888888888"),
            }],
        }
    }

    fn raw_of(o: &V1DutchOrder) -> RawOrder {
        RawOrder::new(
            ProtocolId::UniswapXV1,
            ChainId(1),
            Bytes::from(o.abi_encode()),
            Bytes::from(vec![0xab; 65]),
            1234,
            OrderSource::UniswapX,
        )
    }

    #[test]
    fn normalizes_a_plain_order_with_no_exclusivity() {
        let swapper = key(0x11).address();
        let o = order(swapper, Address::ZERO, 0);
        let normalizer = UniswapXV1Normalizer::new(REACTOR);
        let intent = normalizer.normalize(&raw_of(&o)).expect("normalizes");
        assert_eq!(intent.swapper, swapper);
        assert_eq!(intent.settler, REACTOR);
        assert_eq!(intent.protocol, ProtocolId::UniswapXV1);
        assert_eq!(intent.exclusivity, None);
    }

    #[test]
    fn normalizes_an_exclusive_order() {
        let swapper = key(0x11).address();
        let exclusive_filler = address!("5555555555555555555555555555555555555555");
        let o = order(swapper, exclusive_filler, 100);
        let normalizer = UniswapXV1Normalizer::new(REACTOR);
        let intent = normalizer.normalize(&raw_of(&o)).expect("normalizes");
        assert_eq!(
            intent.exclusivity,
            Some(Exclusivity::new(exclusive_filler, 1000, 100))
        );
    }

    /// Real live `Limit` orders are commonly fully flat (no price movement at all), which means
    /// `decayStartTime == decayEndTime` — a zero-width window. `DutchDecayLib.decay` special-cases
    /// `startAmount == endAmount` before it ever looks at the window, so the real reactor never
    /// reverts on this; the normalizer must not reject it either.
    #[test]
    fn a_fully_flat_order_with_a_zero_width_window_is_accepted() {
        let mut o = order(key(0x11).address(), Address::ZERO, 0);
        o.decayEndTime = o.decayStartTime;
        o.outputs[0].endAmount = o.outputs[0].startAmount; // flat, not decaying
        let normalizer = UniswapXV1Normalizer::new(REACTOR);
        let intent = normalizer
            .normalize(&raw_of(&o))
            .expect("a flat order needs no valid decay window");
        assert_eq!(
            intent.outputs[0].curve.amount_at(0),
            AmountCurve::scalar(o.outputs[0].startAmount).amount_at(0)
        );
    }

    #[test]
    fn rejects_another_deployments_reactor() {
        let mut o = order(key(0x11).address(), Address::ZERO, 0);
        o.info.reactor = Address::repeat_byte(0x99);
        let normalizer = UniswapXV1Normalizer::new(REACTOR);
        assert!(matches!(
            normalizer.normalize(&raw_of(&o)),
            Err(NormalizeError::Decode(_))
        ));
    }

    #[test]
    fn rejects_an_additional_validation_hook() {
        let mut o = order(key(0x11).address(), Address::ZERO, 0);
        o.info.additionalValidationContract = Address::repeat_byte(0x77);
        let normalizer = UniswapXV1Normalizer::new(REACTOR);
        assert!(matches!(
            normalizer.normalize(&raw_of(&o)),
            Err(NormalizeError::Decode(_))
        ));
    }

    #[test]
    fn rejects_a_decay_window_that_does_not_advance() {
        let mut o = order(key(0x11).address(), Address::ZERO, 0);
        o.decayEndTime = o.decayStartTime;
        let normalizer = UniswapXV1Normalizer::new(REACTOR);
        assert!(matches!(
            normalizer.normalize(&raw_of(&o)),
            Err(NormalizeError::Decode(_))
        ));
    }

    #[test]
    fn rejects_an_output_that_decays_upward() {
        let mut o = order(key(0x11).address(), Address::ZERO, 0);
        o.outputs[0].endAmount = o.outputs[0].startAmount + U256::from(1u64);
        let normalizer = UniswapXV1Normalizer::new(REACTOR);
        assert!(matches!(
            normalizer.normalize(&raw_of(&o)),
            Err(NormalizeError::Decode(_))
        ));
    }

    #[test]
    fn rejects_an_input_that_decays_downward() {
        let mut o = order(key(0x11).address(), Address::ZERO, 0);
        o.input.endAmount = o.input.startAmount - U256::from(1u64);
        let normalizer = UniswapXV1Normalizer::new(REACTOR);
        assert!(matches!(
            normalizer.normalize(&raw_of(&o)),
            Err(NormalizeError::Decode(_))
        ));
    }

    #[test]
    fn rejects_a_deadline_before_the_decay_ends() {
        let mut o = order(key(0x11).address(), Address::ZERO, 0);
        o.info.deadline = U256::from(1u64);
        let normalizer = UniswapXV1Normalizer::new(REACTOR);
        assert!(matches!(
            normalizer.normalize(&raw_of(&o)),
            Err(NormalizeError::Decode(_))
        ));
    }

    /// A real order fetched from the live Orders API (`orderType=Limit`), decoded here to pin the
    /// wire format against real data rather than a guessed layout — an earlier draft assumed the
    /// plain (non-exclusive) `DutchOrderLib` struct, which is 2 fields short of what real orders
    /// on this reactor actually carry (`exclusiveFiller`/`exclusivityOverrideBps`).
    #[test]
    fn decodes_a_real_live_limit_order() {
        let payload: Bytes = concat!(
            "0x0000000000000000000000000000000000000000000000000000000000000020",
            "0000000000000000000000000000000000000000000000000000000000000120",
            "000000000000000000000000000000000000000000000000000000006aa3b1fb",
            "000000000000000000000000000000000000000000000000000000006aa3b1fb",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
            "00000000000000000000000000000000000000000000000000000000004c4b40",
            "00000000000000000000000000000000000000000000000000000000004c4b40",
            "0000000000000000000000000000000000000000000000000000000000000200",
            "0000000000000000000000006000da47483062a0d734ba3dc7576ce6a0b645c4",
            "000000000000000000000000fb48f76177307ab9ba9b7620b87662a591430a15",
            "04683255f9d0b7a4e602212dcda448728b0e1be7715641ff38babcf6163d6253",
            "000000000000000000000000000000000000000000000000000000006aacec7b",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "00000000000000000000000000000000000000000000000000000000000000c0",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000001",
            "000000000000000000000000af05ce8a2cef336006e933c02fc89887f5b3c726",
            "00000000000000000000000000000000000000000000050ab784ec7f90061861",
            "00000000000000000000000000000000000000000000050ab784ec7f90061861",
            "000000000000000000000000fb48f76177307ab9ba9b7620b87662a591430a15",
        )
        .parse()
        .expect("valid hex");
        let order = V1DutchOrder::abi_decode(&payload).expect("decodes the real wire format");
        assert_eq!(order.info.reactor, REACTOR);
        assert_eq!(
            order.info.swapper,
            address!("fb48f76177307ab9ba9b7620b87662a591430a15")
        );
        assert_eq!(
            order.info.nonce,
            "1993350894291445726961727631973616900171470509865646750671508638626043748947"
                .parse::<U256>()
                .unwrap()
        );
        assert_eq!(order.info.additionalValidationContract, Address::ZERO);
        assert_eq!(
            order.decayStartTime, order.decayEndTime,
            "a flat Limit order"
        );
        assert_eq!(order.exclusiveFiller, Address::ZERO);
        assert_eq!(order.exclusivityOverrideBps, U256::ZERO);
        assert_eq!(
            order.input.token,
            address!("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48")
        );
        assert_eq!(order.input.startAmount, U256::from(5_000_000u64));
        assert_eq!(order.input.endAmount, U256::from(5_000_000u64));
        assert_eq!(order.outputs.len(), 1);
        assert_eq!(
            order.outputs[0].token,
            address!("af05ce8a2cef336006e933c02fc89887f5b3c726")
        );
        assert_eq!(order.outputs[0].recipient, order.info.swapper);
    }
}
