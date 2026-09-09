//! The canonical intent — the protocol-agnostic order every downstream slice (routing, ledger,
//! execution) consumes. Protocol-specific bytes ride along opaquely in `raw`, decoded only by the
//! protocol's own adapter, so adding a protocol is a new adapter with no change here.

use alloy_primitives::{Address, Bytes};

use crate::primitives::ingest::curve::AmountCurve;
use crate::primitives::{ChainId, IntentId};

/// The source protocol of an intent; selects the normalizer that produced it and the fill builder
/// that will consume `raw`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

/// An exclusive-fill window. Until `ends_at`, `filler` fills at face value and anyone else pays
/// `override_bps` more on every output; `override_bps == 0` makes the window strict, and nobody
/// else can fill at all. `filler` is matched against the reactor's `msg.sender`, so it is the
/// resolver's filler *contract*, not the operator EOA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Exclusivity {
    pub filler: Address,
    pub ends_at: u64,
    pub override_bps: u16,
}

impl Exclusivity {
    pub fn new(filler: Address, ends_at: u64, override_bps: u16) -> Exclusivity {
        Exclusivity {
            filler,
            ends_at,
            override_bps,
        }
    }

    /// Whether `filler` may fill at time `t` without paying the override — the reactor's
    /// `ExclusivityLib.hasFillingRights`, whose window comparison is strictly greater, so the
    /// window still holds *at* `ends_at`.
    pub fn grants_rights_to(&self, candidate: Address, t: u64) -> bool {
        t > self.ends_at || self.filler == candidate
    }
}

/// A normalized, protocol-agnostic order.
///
/// Built through [`Intent::new`] from an [`IntentParts`] literal rather than a positional argument
/// list: `swapper` and `settler` are both bare addresses, and transposing them is a class of bug no
/// test would catch.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Intent {
    /// The order hash.
    pub id: IntentId,
    pub protocol: ProtocolId,
    /// The account whose funds settle the input side — the reactor pulls from it via Permit2, and
    /// it is the trade's taker. Recovered by the protocol's adapter, since the canonical order does
    /// not otherwise carry it.
    pub swapper: Address,
    pub input: IntentInput,
    pub outputs: Vec<IntentOutput>,
    pub deadline: u64,
    pub exclusivity: Option<Exclusivity>,
    pub settler: Address,
    pub origin_chain: ChainId,
    /// Opaque encoded order, decoded only by the protocol's fill builder.
    pub raw: Bytes,
    /// The swapper's signature over `raw`, carried separately for the fill builder.
    pub signature: Bytes,
    /// Arrival time; routing prices the curves at this instant.
    pub observed_at: u64,
}

/// The fields of an [`Intent`], named at the construction site. Deliberately exhaustive: a new
/// required field should break every builder rather than default silently.
#[derive(Clone, Debug)]
pub struct IntentParts {
    pub id: IntentId,
    pub protocol: ProtocolId,
    pub swapper: Address,
    pub input: IntentInput,
    pub outputs: Vec<IntentOutput>,
    pub deadline: u64,
    pub exclusivity: Option<Exclusivity>,
    pub settler: Address,
    pub origin_chain: ChainId,
    pub raw: Bytes,
    pub signature: Bytes,
    pub observed_at: u64,
}

impl IntentParts {
    /// The fields every intent needs; the rest are set on the returned literal.
    pub fn new(
        id: IntentId,
        protocol: ProtocolId,
        swapper: Address,
        input: IntentInput,
        outputs: Vec<IntentOutput>,
        origin_chain: ChainId,
    ) -> IntentParts {
        IntentParts {
            id,
            protocol,
            swapper,
            input,
            outputs,
            deadline: 0,
            exclusivity: None,
            settler: Address::ZERO,
            origin_chain,
            raw: Bytes::new(),
            signature: Bytes::new(),
            observed_at: 0,
        }
    }
}

impl Intent {
    pub fn new(parts: IntentParts) -> Intent {
        Intent {
            id: parts.id,
            protocol: parts.protocol,
            swapper: parts.swapper,
            input: parts.input,
            outputs: parts.outputs,
            deadline: parts.deadline,
            exclusivity: parts.exclusivity,
            settler: parts.settler,
            origin_chain: parts.origin_chain,
            raw: parts.raw,
            signature: parts.signature,
            observed_at: parts.observed_at,
        }
    }
}
