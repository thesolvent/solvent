//! The canonical intent — the protocol-agnostic order every downstream slice (routing, ledger,
//! execution) consumes. Protocol-specific bytes ride along opaquely in `raw`, decoded only by the
//! protocol's own adapter, so adding a protocol is a new adapter with no change here.

use alloy_primitives::{Address, Bytes};

use crate::primitives::ingest::curve::AmountCurve;
use crate::primitives::{ChainId, IntentId};

/// The source protocol of an intent; selects the normalizer that produced it and the fill builder
/// that will consume `raw`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProtocolId {
    UniswapXV2,
}

/// What the taker pays: the token and its amount over time.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct IntentInput {
    pub token: Address,
    pub curve: AmountCurve,
}

impl IntentInput {
    pub fn new(token: Address, curve: AmountCurve) -> IntentInput {
        IntentInput { token, curve }
    }
}

/// What the taker receives, and where: the token, its amount over time, and the recipient.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct IntentOutput {
    pub token: Address,
    pub curve: AmountCurve,
    pub recipient: Address,
}

impl IntentOutput {
    pub fn new(token: Address, curve: AmountCurve, recipient: Address) -> IntentOutput {
        IntentOutput {
            token,
            curve,
            recipient,
        }
    }
}

/// An exclusive-fill window: until `ends_at`, only `filler` may fill. `filler` is matched against
/// the reactor's `msg.sender`, so it is the resolver's filler *contract*, not the operator EOA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exclusivity {
    pub filler: Address,
    pub ends_at: u64,
}

/// A normalized, protocol-agnostic order.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Intent {
    /// Order hash — dedup key, correlation id, and the routing/reservation id.
    pub id: IntentId,
    pub protocol: ProtocolId,
    pub input: IntentInput,
    pub outputs: Vec<IntentOutput>,
    /// Unix seconds after which the order is dead.
    pub deadline: u64,
    pub exclusivity: Option<Exclusivity>,
    /// The reactor/settler to call when filling.
    pub settler: Address,
    pub origin_chain: ChainId,
    /// The encoded signed order — opaque here, decoded by the protocol's fill builder.
    pub raw: Bytes,
    /// Arrival time; routing prices each curve at this instant.
    pub observed_at: u64,
}

impl Intent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: IntentId,
        protocol: ProtocolId,
        input: IntentInput,
        outputs: Vec<IntentOutput>,
        deadline: u64,
        exclusivity: Option<Exclusivity>,
        settler: Address,
        origin_chain: ChainId,
        raw: Bytes,
        observed_at: u64,
    ) -> Intent {
        Intent {
            id,
            protocol,
            input,
            outputs,
            deadline,
            exclusivity,
            settler,
            origin_chain,
            raw,
            observed_at,
        }
    }
}
