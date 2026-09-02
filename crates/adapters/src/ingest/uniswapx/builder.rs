//! Builds and signs UniswapX V2 Dutch orders the way the real swapper + cosigner would, so our
//! self-hosted feed produces orders the deployed reactor accepts. Two signatures: the swapper's over
//! a Permit2 EIP-712 witness digest, and the cosigner's over a raw (unprefixed) digest — both
//! reproduced exactly from the reactor/Permit2, never via `sign_message` (which adds an EIP-191
//! prefix the reactor's raw `ecrecover` rejects).

use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::signers::{local::PrivateKeySigner, SignerSync};
use alloy::sol_types::{eip712_domain, SolValue};

use solvent_core::primitives::ingest::{ProtocolId, RawOrder};
use solvent_core::primitives::ChainId;

use super::codec::{
    order_hash, CosignerData, DutchInput, DutchOutput, OrderInfo, V2DutchOrder, DUTCH_OUTPUT_TYPE,
    ORDER_INFO_TYPE, V2_DUTCH_ORDER_TYPE,
};

const TOKEN_PERMISSIONS_TYPE: &[u8] = b"TokenPermissions(address token,uint256 amount)";
const WITNESS_STUB: &[u8] = b"PermitWitnessTransferFrom(TokenPermissions permitted,address spender,uint256 nonce,uint256 deadline,";
const WITNESS_ORDER_PREFIX: &[u8] = b"V2DutchOrder witness)";

/// The parameters of one exact-input Dutch order to mint. A `Static` input (`input_start ==
/// input_end`) and a decaying single output is the common shape.
pub struct OrderSpec {
    pub reactor: Address,
    pub nonce: U256,
    pub deadline: u64,
    pub input_token: Address,
    pub input_start: U256,
    pub input_end: U256,
    pub output_token: Address,
    pub output_start: U256,
    pub output_end: U256,
    pub recipient: Address,
    pub decay_start: u64,
    pub decay_end: u64,
    /// `Address::ZERO` for an open (non-exclusive) order.
    pub exclusive_filler: Address,
}

/// Mints signed+cosigned orders against a fixed Permit2 + chain, holding the swapper and cosigner keys.
pub struct SignedOrderBuilder {
    permit2: Address,
    chain_id: u64,
    swapper: PrivateKeySigner,
    cosigner: PrivateKeySigner,
}

impl SignedOrderBuilder {
    pub fn new(
        permit2: Address,
        chain_id: u64,
        swapper: PrivateKeySigner,
        cosigner: PrivateKeySigner,
    ) -> SignedOrderBuilder {
        SignedOrderBuilder {
            permit2,
            chain_id,
            swapper,
            cosigner,
        }
    }

    /// A `RawOrder` carrying the encoded cosigned order plus the swapper's separate signature.
    pub fn build(&self, spec: &OrderSpec, observed_at: u64) -> RawOrder {
        let mut order = V2DutchOrder {
            info: OrderInfo {
                reactor: spec.reactor,
                swapper: self.swapper.address(),
                nonce: spec.nonce,
                deadline: U256::from(spec.deadline),
                additionalValidationContract: Address::ZERO,
                additionalValidationData: Bytes::new(),
            },
            cosigner: self.cosigner.address(),
            baseInput: DutchInput {
                token: spec.input_token,
                startAmount: spec.input_start,
                endAmount: spec.input_end,
            },
            baseOutputs: vec![DutchOutput {
                token: spec.output_token,
                startAmount: spec.output_start,
                endAmount: spec.output_end,
                recipient: spec.recipient,
            }],
            cosignerData: CosignerData {
                decayStartTime: U256::from(spec.decay_start),
                decayEndTime: U256::from(spec.decay_end),
                exclusiveFiller: spec.exclusive_filler,
                exclusivityOverrideBps: U256::ZERO,
                inputAmount: U256::ZERO,
                // One entry per output (the reactor requires the lengths match); 0 = defer to the
                // base decay, no cosigner override.
                outputAmounts: vec![U256::ZERO],
            },
            cosignature: Bytes::new(),
        };

        let hash = order_hash(&order);
        order.cosignature = sign65(&self.cosigner, cosign_digest(&order, hash));
        let signature = sign65(
            &self.swapper,
            witness_digest(&order, hash, self.permit2, self.chain_id),
        );

        RawOrder::new(
            ProtocolId::UniswapXV2,
            ChainId(self.chain_id),
            Bytes::from(order.abi_encode()),
            signature,
            observed_at,
        )
    }
}

/// The cosigner's preimage: `keccak(orderHash ‖ abi.encode(cosignerData))`, signed raw (no EIP-191).
pub(crate) fn cosign_digest(order: &V2DutchOrder, order_hash: B256) -> B256 {
    keccak256([order_hash.as_slice(), &order.cosignerData.abi_encode()].concat())
}

/// The swapper's Permit2 EIP-712 witness digest (`toTypedDataHash`), with the order hash as witness
/// and `permitted.amount = baseInput.endAmount`.
pub(crate) fn witness_digest(
    order: &V2DutchOrder,
    order_hash: B256,
    permit2: Address,
    chain_id: u64,
) -> B256 {
    let domain_separator = eip712_domain! {
        name: "Permit2",
        chain_id: chain_id,
        verifying_contract: permit2,
    }
    .separator();
    let permit2_order_type = [
        WITNESS_ORDER_PREFIX,
        DUTCH_OUTPUT_TYPE,
        ORDER_INFO_TYPE,
        TOKEN_PERMISSIONS_TYPE,
        V2_DUTCH_ORDER_TYPE,
    ]
    .concat();
    let witness_type_hash = keccak256([WITNESS_STUB, &permit2_order_type].concat());
    let token_permissions = keccak256(
        (
            keccak256(TOKEN_PERMISSIONS_TYPE),
            order.baseInput.token,
            order.baseInput.endAmount,
        )
            .abi_encode(),
    );
    let struct_hash = keccak256(
        (
            witness_type_hash,
            token_permissions,
            order.info.reactor,
            order.info.nonce,
            order.info.deadline,
            order_hash,
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

/// A 65-byte `r ‖ s ‖ v` signature with `v ∈ {27, 28}`, as the reactor's `ecrecover` expects
/// (alloy's `as_bytes` lays it out exactly so).
fn sign65(signer: &PrivateKeySigner, digest: B256) -> Bytes {
    let sig = signer
        .sign_hash_sync(&digest)
        .expect("a local signer signs a 32-byte digest infallibly");
    Bytes::from(sig.as_bytes())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use alloy::primitives::{address, b256, Signature};
    use alloy::sol_types::SolValue;

    fn signer(byte: u8) -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::from([byte; 32])).expect("valid test key")
    }

    // The fixture order and its digests come from contracts/script/GenV2OrderFixture.s.sol.
    #[test]
    fn digests_match_the_contract() {
        let payload =
            Bytes::from_str(include_str!("../../../tests/fixtures/uniswapx_v2_order.hex").trim())
                .expect("fixture hex");
        let order = V2DutchOrder::abi_decode(&payload).expect("round-trips");
        let hash = order_hash(&order);
        let permit2 = address!("000000000022D473030F116dDEE9F6B43aC78BA3");

        assert_eq!(
            cosign_digest(&order, hash),
            b256!("c0fb93c8a3b8d5bb2eaf22c8e115b8240fb800dc4a4ea102978f6253d8f6c5ff")
        );
        assert_eq!(
            witness_digest(&order, hash, permit2, 1),
            b256!("3b88450b974198dfb2ff74e1bd468880355a750753557de07c60419c19c3e836")
        );
    }

    fn spec() -> OrderSpec {
        OrderSpec {
            reactor: address!("2222222222222222222222222222222222222222"),
            nonce: U256::from(42u64),
            deadline: 2000,
            input_token: address!("6666666666666666666666666666666666666666"),
            input_start: U256::from(500u64),
            input_end: U256::from(500u64),
            output_token: address!("7777777777777777777777777777777777777777"),
            output_start: U256::from(1000u64),
            output_end: U256::from(900u64),
            recipient: address!("8888888888888888888888888888888888888888"),
            decay_start: 1000,
            decay_end: 1100,
            exclusive_filler: Address::ZERO,
        }
    }

    fn recover(sig: &Bytes, digest: B256) -> Address {
        let r = U256::from_be_slice(&sig[0..32]);
        let s = U256::from_be_slice(&sig[32..64]);
        let sig = Signature::new(r, s, sig[64] == 28);
        sig.recover_address_from_prehash(&digest)
            .expect("recoverable")
    }

    #[test]
    fn both_signatures_recover_to_their_signers() {
        let permit2 = address!("000000000022D473030F116dDEE9F6B43aC78BA3");
        let swapper = signer(0x11);
        let cosigner = signer(0x22);
        let builder = SignedOrderBuilder::new(permit2, 1, swapper.clone(), cosigner.clone());

        let raw = builder.build(&spec(), 1234);
        let order = V2DutchOrder::abi_decode(&raw.payload).expect("round-trips");
        let hash = order_hash(&order);

        // The reactor's ecrecover needs v ∈ {27, 28}.
        assert!(matches!(raw.signature[64], 27 | 28));
        assert!(matches!(order.cosignature[64], 27 | 28));

        assert_eq!(
            recover(&order.cosignature, cosign_digest(&order, hash)),
            cosigner.address()
        );
        assert_eq!(
            recover(&raw.signature, witness_digest(&order, hash, permit2, 1)),
            swapper.address()
        );
    }
}
