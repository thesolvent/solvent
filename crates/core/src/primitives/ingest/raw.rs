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
    /// Which protocol's encoding `payload` is — selects the normalizer and fill builder.
    pub protocol: ProtocolId,
    pub chain: ChainId,
    /// The encoded signed order, opaque until the protocol's normalizer decodes it.
    pub payload: Bytes,
    /// Arrival time, stamped by the feed; becomes `Intent::observed_at`.
    pub observed_at: u64,
}

impl RawOrder {
    pub fn new(protocol: ProtocolId, chain: ChainId, payload: Bytes, observed_at: u64) -> RawOrder {
        RawOrder {
            protocol,
            chain,
            payload,
            observed_at,
        }
    }
}
