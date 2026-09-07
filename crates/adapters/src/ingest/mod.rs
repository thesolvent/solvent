//! Ingest adapters: per-protocol normalizers, order builders and feeds.

use alloy::primitives::{Bytes, B256};
use alloy::signers::{local::PrivateKeySigner, SignerSync};

pub mod uniswapx;

/// A 65-byte `r ‖ s ‖ v` signature with `v ∈ {27, 28}`, as on-chain `ecrecover` expects (alloy's
/// `as_bytes` lays it out exactly so). Shared by the protocol order builders.
pub(crate) fn sign65(signer: &PrivateKeySigner, digest: B256) -> Bytes {
    let sig = signer
        .sign_hash_sync(&digest)
        .expect("a local signer signs a 32-byte digest infallibly");
    Bytes::from(sig.as_bytes())
}
