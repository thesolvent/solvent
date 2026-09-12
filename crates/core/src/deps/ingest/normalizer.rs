//! The normalizer port: decode one protocol's raw order into the canonical `Intent`. Pure and
//! synchronous — decoding touches no I/O; any on-chain resolution a protocol needs happens upstream
//! in its feed adapter, which hands this a decodable `RawOrder`.

use thiserror::Error;

use crate::primitives::ingest::{Intent, ProtocolId, RawOrder};

/// Decodes a `RawOrder` of one protocol into an `Intent`. One implementation per protocol, selected
/// by `RawOrder::protocol`.
pub trait Normalizer: Send + Sync {
    fn normalize(&self, raw: &RawOrder) -> Result<Intent, NormalizeError>;
}

/// A normalization failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NormalizeError {
    /// The payload did not decode as a well-formed order of this protocol.
    #[error("malformed {0:?} order")]
    Decode(ProtocolId),
    #[error("invalid {0:?} order")]
    Invalid(ProtocolId),
    #[error("invalid {0:?} order signature")]
    Signature(ProtocolId),
}
