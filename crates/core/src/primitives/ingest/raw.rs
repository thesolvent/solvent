//! A raw, undecoded order as it leaves a feed: protocol-tagged opaque bytes plus the little the
//! pipeline needs before decode (which chain, when it arrived). The normalizer turns it into an
//! `Intent`; the bytes become `Intent::raw`.

use alloy_primitives::Bytes;

use crate::primitives::ingest::intent::ProtocolId;
use crate::primitives::ChainId;

/// Where an order reached this resolver. Distinct from [`ProtocolId`], which selects the decoder:
/// the public feed and this resolver's own submit endpoint both carry UniswapX V2 orders, and only
/// the venue tells them apart — which matters because they are cosigned by different keys, compete
/// differently, and one of them is flow we were given rather than flow we found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum OrderSource {
    /// Polled from the protocol's public order book.
    UniswapX,
    /// Submitted straight to this resolver by a taker.
    Solvent,
}

impl OrderSource {
    /// The stable wire label, as the API and the explorer render it.
    pub fn as_str(&self) -> &'static str {
        match self {
            OrderSource::UniswapX => "uniswapx",
            OrderSource::Solvent => "solvent",
        }
    }
}

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
    /// Where it came from — the venue, not the protocol.
    pub source: OrderSource,
}

impl RawOrder {
    pub fn new(
        protocol: ProtocolId,
        chain: ChainId,
        payload: Bytes,
        signature: Bytes,
        observed_at: u64,
        source: OrderSource,
    ) -> RawOrder {
        RawOrder {
            protocol,
            chain,
            payload,
            signature,
            observed_at,
            source,
        }
    }
}
