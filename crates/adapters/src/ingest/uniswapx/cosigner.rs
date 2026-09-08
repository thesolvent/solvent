//! Cosigns a taker-signed UniswapX V2 order. The taker (swapper) builds and signs the base order
//! client-side; the resolver verifies that signature, then applies its own cosigner signature (the
//! decay window + exclusivity) — producing the `RawOrder` the normalizer and reactor accept. The
//! server holds only its own cosigner key, never the swapper's.

use alloy::primitives::{Address, Bytes, Signature, U256};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolValue;
use thiserror::Error;

use solvent_core::primitives::ingest::{ProtocolId, RawOrder};
use solvent_core::primitives::ChainId;

use super::builder::{cosign_digest, sign65, witness_digest};
use super::codec::{order_hash, V2DutchOrder};

/// Applies this resolver's cosignature to taker-signed orders, over a fixed Permit2 + chain. Holds
/// the cosigner key only (never a swapper key), so it is not `Debug` (the key must not print).
pub struct ServerCosigner {
    permit2: Address,
    chain_id: u64,
    cosigner: PrivateKeySigner,
    exclusive_filler: Address,
    decay_window_secs: u64,
}

/// A cosigned order plus the swapper the signature recovered to (the trade's taker).
pub(crate) struct Cosigned {
    pub swapper: Address,
    pub raw: RawOrder,
}

impl ServerCosigner {
    pub fn new(
        permit2: Address,
        chain_id: u64,
        cosigner: PrivateKeySigner,
        exclusive_filler: Address,
        decay_window_secs: u64,
    ) -> ServerCosigner {
        ServerCosigner {
            permit2,
            chain_id,
            cosigner,
            exclusive_filler,
            decay_window_secs,
        }
    }

    /// The cosigner's on-chain address — the value a taker must name as the order's `cosigner`.
    pub fn address(&self) -> Address {
        self.cosigner.address()
    }

    /// Verify the swapper's signature over the base order, then apply our cosignature (decay window
    /// anchored at `observed_at`). `signature` never reaches a log, and the key never leaves this fn.
    #[tracing::instrument(skip_all)]
    pub(crate) fn cosign(
        &self,
        encoded_order: &[u8],
        signature: Bytes,
        observed_at: u64,
    ) -> Result<Cosigned, CosignError> {
        let mut order = V2DutchOrder::abi_decode(encoded_order).map_err(|_| CosignError::Decode)?;
        if order.cosigner != self.cosigner.address() {
            return Err(CosignError::WrongCosigner);
        }
        // The swapper signs the base order (the witness excludes cosignerData), so verifying it here
        // and setting the decay afterwards keeps the signature valid.
        let hash = order_hash(&order);
        let digest = witness_digest(&order, hash, self.permit2, self.chain_id);
        let signature = match signature.last() {
            Some(0 | 1 | 27 | 28) => {
                Signature::from_raw(&signature).map_err(|_| CosignError::BadSignature)?
            }
            _ => return Err(CosignError::BadSignature),
        };
        let swapper = signature
            .recover_address_from_prehash(&digest)
            .map_err(|_| CosignError::BadSignature)?;
        if swapper != order.info.swapper {
            return Err(CosignError::BadSignature);
        }

        order.cosignerData.decayStartTime = U256::from(observed_at);
        order.cosignerData.decayEndTime =
            U256::from(observed_at.saturating_add(self.decay_window_secs));
        order.cosignerData.exclusiveFiller = self.exclusive_filler;
        order.cosignerData.exclusivityOverrideBps = U256::ZERO;
        order.cosignerData.inputAmount = U256::ZERO;
        order.cosignerData.outputAmounts = vec![U256::ZERO; order.baseOutputs.len()];
        order.cosignature = sign65(&self.cosigner, cosign_digest(&order, hash));

        Ok(Cosigned {
            swapper,
            raw: RawOrder::new(
                ProtocolId::UniswapXV2,
                ChainId(self.chain_id),
                Bytes::from(order.abi_encode()),
                Bytes::from(signature.as_bytes()),
                observed_at,
            ),
        })
    }
}

/// A cosigning failure — all client-input (a malformed or unverifiable order), never infra.
#[derive(Debug, Error)]
#[non_exhaustive]
pub(crate) enum CosignError {
    #[error("malformed order")]
    Decode,
    #[error("order names a different cosigner")]
    WrongCosigner,
    #[error("invalid swapper signature")]
    BadSignature,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::uniswapx::{OrderSpec, SignedOrderBuilder};
    use alloy::primitives::{address, B256};
    use alloy::signers::local::PrivateKeySigner;

    const PERMIT2: Address = address!("000000000022D473030F116dDEE9F6B43aC78BA3");

    fn key(byte: u8) -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::from([byte; 32])).expect("test key")
    }

    /// A swapper-signed base order (cosigner set to `cosigner`, ready for the resolver to cosign),
    /// reusing the builder's swapper-witness signing.
    fn base_order(swapper: &PrivateKeySigner, cosigner: Address) -> (Bytes, Bytes) {
        // The builder signs both; we take its swapper signature and re-zero the cosignerData so the
        // base is what a taker would post.
        let builder = SignedOrderBuilder::new(PERMIT2, 31337, swapper.clone(), key(0x99));
        let spec = OrderSpec {
            reactor: address!("2222222222222222222222222222222222222222"),
            nonce: U256::from(1u64),
            deadline: 2_000_000_000,
            input_token: address!("6666666666666666666666666666666666666666"),
            input_start: U256::from(500u64),
            input_end: U256::from(500u64),
            output_token: address!("7777777777777777777777777777777777777777"),
            output_start: U256::from(1000u64),
            output_end: U256::from(1000u64),
            recipient: address!("8888888888888888888888888888888888888888"),
            decay_start: 0,
            decay_end: 0,
            exclusive_filler: Address::ZERO,
        };
        let raw = builder.build(&spec, 0);
        let mut order = V2DutchOrder::abi_decode(&raw.payload).expect("decode");
        order.cosigner = cosigner; // the swapper names our cosigner
        order.cosignerData.outputAmounts = vec![U256::ZERO];
        order.cosignature = Bytes::new();
        // Re-sign the swapper witness over the (now cosigner-corrected) base order.
        let hash = order_hash(&order);
        let signature = sign65(swapper, witness_digest(&order, hash, PERMIT2, 31337));
        (Bytes::from(order.abi_encode()), signature)
    }

    #[test]
    fn cosigns_a_valid_order() {
        let swapper = key(0x11);
        let cosigner = key(0x22);
        let server = ServerCosigner::new(PERMIT2, 31337, cosigner.clone(), Address::ZERO, 60);
        let (encoded, sig) = base_order(&swapper, cosigner.address());

        let out = server.cosign(&encoded, sig, 1000).expect("cosign");
        assert_eq!(out.swapper, swapper.address());
        // The cosigned order now carries a cosignature and the decay window.
        let order = V2DutchOrder::abi_decode(&out.raw.payload).unwrap();
        assert_eq!(order.cosignature.len(), 65);
        assert_eq!(order.cosignerData.decayStartTime, U256::from(1000u64));
        assert_eq!(order.cosignerData.decayEndTime, U256::from(1060u64));
    }

    #[test]
    fn normalizes_wallet_parity_for_onchain_verification() {
        let cosigner = key(0x22);
        let server = ServerCosigner::new(PERMIT2, 31337, cosigner.clone(), Address::ZERO, 60);
        let mut seen = [false; 2];
        for byte in 1..=16 {
            let swapper = key(byte);
            let (encoded, canonical) = base_order(&swapper, cosigner.address());
            let mut wallet_signature = canonical.to_vec();
            wallet_signature[64] -= 27;
            seen[wallet_signature[64] as usize] = true;
            let out = server
                .cosign(&encoded, wallet_signature.into(), 1000)
                .expect("cosign");
            assert_eq!(out.swapper, swapper.address());
            assert_eq!(out.raw.signature, canonical);
        }
        assert_eq!(seen, [true, true]);
    }

    #[test]
    fn rejects_a_bad_swapper_signature() {
        let swapper = key(0x11);
        let cosigner = key(0x22);
        let server = ServerCosigner::new(PERMIT2, 31337, cosigner.clone(), Address::ZERO, 60);
        let (encoded, _) = base_order(&swapper, cosigner.address());
        // A signature from the wrong key does not recover to the order's swapper.
        let wrong = base_order(&key(0x33), cosigner.address()).1;
        assert!(matches!(
            server.cosign(&encoded, wrong, 1000),
            Err(CosignError::BadSignature)
        ));
    }

    #[test]
    fn rejects_an_order_for_another_cosigner() {
        let swapper = key(0x11);
        let server = ServerCosigner::new(PERMIT2, 31337, key(0x22), Address::ZERO, 60);
        // Order names a different cosigner (0x44).
        let (encoded, sig) = base_order(&swapper, key(0x44).address());
        assert!(matches!(
            server.cosign(&encoded, sig, 1000),
            Err(CosignError::WrongCosigner)
        ));
    }
}
