//! The simulation-gate port: check the exact fill against live state and reject it before a nonce
//! is spent. The live impl eth-calls the fill (a would-revert is a `Reject`).

use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::execution::{FillTx, SimVerdict};

/// Simulates a fill and returns whether it may be submitted. The gate is a pre-nonce guard, so an
/// engine that cannot confirm success rejects rather than risks a wasted nonce.
#[async_trait]
pub trait SimGate: Send + Sync {
    async fn simulate(&self, fill: &FillTx) -> Result<SimVerdict, SimError>;
}

/// A failure of the simulation engine itself (an RPC read failed) — distinct from a fill that
/// simulates cleanly to a `Reject`.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SimError {
    #[error("simulation engine: {0}")]
    Engine(String),
}
