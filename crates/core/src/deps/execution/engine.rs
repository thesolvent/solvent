//! The execution port: submit a reserved fill to the chain and track it to a terminal status. The
//! live impl wraps the walletkit tx engine (sign → private submit → confirm / bump / reorg); a
//! backtest impl simulates deterministic inclusion against the replay chain.

use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::execution::{ExecHandle, ExecStatus, FillTx, TrackedFill};
use crate::primitives::{IntentId, ReservationId};

/// Submits a fill transaction and reports its lifecycle. Tracking is durable — a submitted fill and
/// its reservation survive a restart, so [`tracked`](Execution::tracked) is the recovery read and
/// the reconcile working set at once. [`tick`](Execution::tick) drives one confirm/bump pass over
/// all in-flight fills; [`status`](Execution::status) reads a tracked fill's current state (`None`
/// if it is not tracked).
#[async_trait]
pub trait Execution: Send + Sync {
    /// Submit a reserved fill, durably recording the intent → reservation → tracking handle so a
    /// restart can recover it. Idempotent on the intent: resubmitting a tracked fill returns its
    /// handle without spending a second nonce.
    async fn submit(
        &self,
        fill: &FillTx,
        reservation: ReservationId,
    ) -> Result<ExecHandle, ExecutionError>;
    async fn status(&self, handle: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError>;
    /// Stop tracking a fill that reached a terminal state, dropping its durable record.
    async fn forget(&self, intent: IntentId) -> Result<(), ExecutionError>;
    /// The fills still tracked as in-flight — the reconcile working set, and after a restart the
    /// fills to recover.
    async fn tracked(&self) -> Result<Vec<TrackedFill>, ExecutionError>;
    async fn tick(&self) -> Result<(), ExecutionError>;
}

/// A failure surfaced by the tx engine. The underlying error is kept as its (already-redacted)
/// display string — core cannot depend on the engine's own error type.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExecutionError {
    #[error("execution engine: {0}")]
    Engine(String),
}
