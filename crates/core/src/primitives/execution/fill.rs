//! A routed, reserved plan becomes a [`FillTx`] — the raw transaction the execution port submits —
//! and comes back as an [`ExecStatus`] the service couples to the ledger.

use alloy_primitives::{Address, Bytes, B256};

use crate::primitives::{IntentId, ReservationId};

/// The transaction that settles one reserved plan on-chain: a call to a protocol's filler contract
/// carrying the calldata its `FillBuilder` produced. Protocol-agnostic — the adapter turns it into
/// the tx engine's own intent type and chooses the broadcast route.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FillTx {
    /// The intent this fill settles — the correlation and idempotency key.
    pub intent: IntentId,
    pub chain_id: u64,
    /// The `onlyOwner` account authorized to call the filler.
    pub filler_owner: Address,
    /// The filler contract the transaction targets.
    pub filler: Address,
    /// ABI-encoded `fill(...)` calldata from the protocol's `FillBuilder`.
    pub calldata: Bytes,
}

impl FillTx {
    pub fn new(
        intent: IntentId,
        chain_id: u64,
        filler_owner: Address,
        filler: Address,
        calldata: Bytes,
    ) -> Self {
        Self {
            intent,
            chain_id,
            filler_owner,
            filler,
            calldata,
        }
    }
}

/// A submitted fill's lifecycle, projected from the tx engine to just what the reservation coupling
/// needs: the terminal variants decide `post` vs `void`, and `Pending` covers every not-yet-settled
/// engine state (including reorg re-tracking, which the engine absorbs below confirmation depth).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExecStatus {
    Pending,
    /// Reached the configured confirmation depth. `tx` is the *mined* hash (the latest broadcast,
    /// so it survives an RBF bump) — the settlement reader keys off it.
    Confirmed {
        block: u64,
        tx: B256,
    },
    Failed {
        reason: String,
    },
    Dropped,
}

/// An opaque handle to a submitted fill. The tx engine's own id is not reconstructable from raw
/// bytes, so the adapter keys its tracking by this value; the caller only hands it back to
/// [`status`](crate::deps::execution::Execution::status).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExecHandle(pub B256);

/// The verdict of simulating a fill before it spends a nonce. `Reject` carries the reason so the
/// service can log why a fill was dropped. Fail-closed: only a confirmed success is [`Ok`](SimVerdict::Ok).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SimVerdict {
    Ok,
    Reject { reason: String },
}

/// A reserved plan handed to the execution service to fill: the transaction to send and the
/// reservation whose holds it settles.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PendingFill {
    pub fill_tx: FillTx,
    pub reservation: ReservationId,
}

impl PendingFill {
    pub fn new(fill_tx: FillTx, reservation: ReservationId) -> Self {
        Self {
            fill_tx,
            reservation,
        }
    }
}

/// The result of asking the service to fill a plan: submitted and now tracked, or dropped by the
/// simulation gate (the reservation is voided in that case, having never spent a nonce).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FillOutcome {
    Submitted { handle: ExecHandle },
    Rejected { reason: String },
}
