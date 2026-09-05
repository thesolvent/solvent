//! The execution port: submit a reserved fill to the chain and track it to a terminal status. The
//! live impl wraps the walletkit tx engine (sign → private submit → confirm / bump / reorg); a
//! backtest impl simulates deterministic inclusion against the replay chain.

use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::execution::{ExecHandle, ExecStatus, FillTx};

/// Submits a fill transaction and reports its lifecycle. [`tick`](Execution::tick) drives one
/// confirm/bump pass over all in-flight fills; [`status`](Execution::status) reads a tracked
/// fill's current state (`None` if the handle is unknown).
#[async_trait]
pub trait Execution: Send + Sync {
    async fn submit(&self, fill: &FillTx) -> Result<ExecHandle, ExecutionError>;
    async fn status(&self, handle: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError>;
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
