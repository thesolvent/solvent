//! Builds and signs ERC-7683 gasless orders the way a real swapper would, so the self-hosted feed
//! produces orders `SameChainSettler.openFor` accepts. One signature: the swapper's over a Permit2
//! EIP-712 witness digest whose witness is the order envelope.

use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::{eip712_domain, SolStruct, SolValue};

use solvent_core::primitives::ingest::{ProtocolId, RawOrder};
use solvent_core::primitives::ChainId;

use super::codec::{order_id, solvent_order_type, GaslessCrossChainOrder, SolventOrder};
use crate::ingest::sign65;

const TOKEN_PERMISSIONS_TYPE: &[u8] = b"TokenPermissions(address token,uint256 amount)";
const WITNESS_STUB: &[u8] = b"PermitWitnessTransferFrom(TokenPermissions permitted,address spender,uint256 nonce,uint256 deadline,";
const WITNESS_ORDER_PREFIX: &[u8] = b"GaslessCrossChainOrder witness)";

/// The parameters of one fixed-price order to mint.
pub struct OrderSpec {
    pub settler: Address,
    pub nonce: U256,
    pub open_deadline: u32,
    pub fill_deadline: u32,
    pub input_token: Address,
    pub input_amount: U256,
    pub output_token: Address,
    pub output_amount: U256,
    pub recipient: Address,
    /// `Address::ZERO` for an open (non-exclusive) order.
    pub exclusive_filler: Address,
    pub exclusivity_ends: u32,
}

/// Mints signed orders against a fixed Permit2 + chain, holding the swapper's key.
pub struct SignedOrderBuilder {
    permit2: Address,
    chain_id: u64,
    swapper: PrivateKeySigner,
}

impl SignedOrderBuilder {
    pub fn new(permit2: Address, chain_id: u64, swapper: PrivateKeySigner) -> SignedOrderBuilder {
        SignedOrderBuilder {
            permit2,
            chain_id,
            swapper,
        }
    }

    /// A `RawOrder` carrying the encoded envelope plus the swapper's Permit2 witness signature.
    pub fn build(&self, spec: &OrderSpec, observed_at: u64) -> RawOrder {
        let inner = SolventOrder {
            inputToken: spec.input_token,
            inputAmount: spec.input_amount,
            outputToken: spec.output_token,
            outputAmount: spec.output_amount,
            recipient: spec.recipient,
            exclusiveFiller: spec.exclusive_filler,
            exclusivityEnds: spec.exclusivity_ends,
        };
        let order = GaslessCrossChainOrder {
            originSettler: spec.settler,
            user: self.swapper.address(),
            nonce: spec.nonce,
            originChainId: U256::from(self.chain_id),
            openDeadline: spec.open_deadline,
            fillDeadline: spec.fill_deadline,
            orderDataType: solvent_order_type(),
            orderData: Bytes::from(inner.abi_encode()),
        };
        let signature = sign65(
            &self.swapper,
            witness_digest(
                &order,
                spec.input_token,
                spec.input_amount,
                self.permit2,
                self.chain_id,
            ),
        );

        RawOrder::new(
            ProtocolId::Erc7683,
            ChainId(self.chain_id),
            Bytes::from(order.abi_encode()),
            signature,
            observed_at,
        )
    }
}

/// The swapper's Permit2 EIP-712 witness digest (`toTypedDataHash`), with the order id as witness
/// and `permitted.amount` the escrowed input.
pub(crate) fn witness_digest(
    order: &GaslessCrossChainOrder,
    input_token: Address,
    input_amount: U256,
    permit2: Address,
    chain_id: u64,
) -> B256 {
    let domain_separator = eip712_domain! {
        name: "Permit2",
        chain_id: chain_id,
        verifying_contract: permit2,
    }
    .separator();
    // EIP-712 orders referenced struct types by name: GaslessCrossChainOrder before TokenPermissions.
    let witness_type_hash = keccak256(
        [
            WITNESS_STUB,
            WITNESS_ORDER_PREFIX,
            GaslessCrossChainOrder::eip712_encode_type().as_bytes(),
            TOKEN_PERMISSIONS_TYPE,
        ]
        .concat(),
    );
    let token_permissions =
        keccak256((keccak256(TOKEN_PERMISSIONS_TYPE), input_token, input_amount).abi_encode());
    let struct_hash = keccak256(
        (
            witness_type_hash,
            token_permissions,
            order.originSettler,
            order.nonce,
            U256::from(order.openDeadline),
            order_id(order),
        )
            .abi_encode(),
    );
    keccak256(
        [
            &[0x19u8, 0x01],
            domain_separator.as_slice(),
            struct_hash.as_slice(),
        ]
        .concat(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, b256, Signature};
    use solvent_core::deps::ingest::Normalizer;
    use solvent_core::primitives::ingest::AmountCurve;

    use super::super::Erc7683Normalizer;

    // The fixture and its digest come from contracts/script/GenSolventOrderFixture.s.sol. If this
    // drifts, Permit2 recovers a different signer and every `openFor` reverts.
    #[test]
    fn witness_digest_matches_the_contract() {
        use std::str::FromStr;
        let payload =
            Bytes::from_str(include_str!("../../../tests/fixtures/erc7683_order.hex").trim())
                .expect("fixture hex");
        let order = GaslessCrossChainOrder::abi_decode(&payload).expect("round-trips");
        let inner = SolventOrder::abi_decode(&order.orderData).expect("round-trips");

        assert_eq!(
            witness_digest(
                &order,
                inner.inputToken,
                inner.inputAmount,
                address!("000000000022D473030F116dDEE9F6B43aC78BA3"),
                1
            ),
            b256!("82a930d85e3233b8cc3866234b5c61cd0943b1565c5d6a1acce63c40e4539fdc")
        );
    }

    fn signer(byte: u8) -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::from([byte; 32])).expect("valid test key")
    }

    fn permit2() -> Address {
        address!("000000000022D473030F116dDEE9F6B43aC78BA3")
    }

    fn spec() -> OrderSpec {
        OrderSpec {
            settler: address!("2222222222222222222222222222222222222222"),
            nonce: U256::from(42u64),
            open_deadline: 1500,
            fill_deadline: 2000,
            input_token: address!("6666666666666666666666666666666666666666"),
            input_amount: U256::from(500u64),
            output_token: address!("7777777777777777777777777777777777777777"),
            output_amount: U256::from(1000u64),
            recipient: address!("8888888888888888888888888888888888888888"),
            exclusive_filler: Address::ZERO,
            exclusivity_ends: 0,
        }
    }

    fn recover(sig: &Bytes, digest: B256) -> Address {
        let r = U256::from_be_slice(&sig[0..32]);
        let s = U256::from_be_slice(&sig[32..64]);
        Signature::new(r, s, sig[64] == 28)
            .recover_address_from_prehash(&digest)
            .expect("recoverable")
    }

    #[test]
    fn the_signature_recovers_to_the_swapper() {
        let swapper = signer(0x11);
        let builder = SignedOrderBuilder::new(permit2(), 1, swapper.clone());
        let spec = spec();

        let raw = builder.build(&spec, 1234);
        let order = GaslessCrossChainOrder::abi_decode(&raw.payload).expect("round-trips");

        // Permit2's ecrecover needs v ∈ {27, 28}.
        assert!(matches!(raw.signature[64], 27 | 28));
        assert_eq!(
            recover(
                &raw.signature,
                witness_digest(&order, spec.input_token, spec.input_amount, permit2(), 1)
            ),
            swapper.address()
        );
    }

    // The builder is the feed's only order source, so what it mints must survive the normalizer.
    #[test]
    fn what_it_builds_normalizes() {
        let builder = SignedOrderBuilder::new(permit2(), 1, signer(0x11));
        let raw = builder.build(&spec(), 1234);

        let intent = Erc7683Normalizer.normalize(&raw).expect("normalizes");
        assert_eq!(intent.protocol, ProtocolId::Erc7683);
        assert_eq!(intent.input.curve, AmountCurve::scalar(U256::from(500u64)));
        assert_eq!(intent.deadline, 2000);
        assert_eq!(intent.origin_chain, ChainId(1));
    }
}
