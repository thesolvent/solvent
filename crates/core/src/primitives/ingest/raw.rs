//! A raw, undecoded order as it leaves a feed: protocol-tagged opaque bytes plus the little the
//! pipeline needs before decode (which chain, when it arrived). The normalizer turns it into an
//! `Intent`; the bytes become `Intent::raw`.

use alloy_primitives::Bytes;

use crate::primitives::ingest::intent::ProtocolId;
use crate::primitives::ChainId;

/// An order straight off a feed, before normalization.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RawOrder {
    /// Selects the normalizer for `payload`.
    pub protocol: ProtocolId,
    pub chain: ChainId,
    /// Opaque encoded order, until the protocol's normalizer decodes it.
    pub payload: Bytes,
    /// The swapper's signature, carried separately as the feed delivers it.
    pub signature: Bytes,
    pub observed_at: u64,
}

impl RawOrder {
    pub fn new(
        protocol: ProtocolId,
        chain: ChainId,
        payload: Bytes,
        signature: Bytes,
        observed_at: u64,
    ) -> RawOrder {
        RawOrder {
            protocol,
            chain,
            payload,
            signature,
            observed_at,
        }
    }
}
