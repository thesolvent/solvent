//! The chain-source port: pull Aqua events for a block range.

use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::registry::{AquaEvent, EventExt};

/// Fetches the Aqua events emitted in a block range, in canonical `(block,
/// log_index)` order. Poll-shaped (the MVP `eth_getLogs` design) — a future
/// HyperSync or WebSocket source implements the same trait.
#[async_trait]
pub trait ChainSource: Send + Sync {
    /// Aqua events in `[from_block, to_block]`, sorted by chain position, each
    /// tagged with the provenance of the log it came from.
    async fn fetch(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<EventExt<AquaEvent>>, ChainSourceError>;
}

/// A chain-source failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ChainSourceError {
    /// The RPC call failed (after the adapter's own retries).
    #[error("chain rpc: {0}")]
    Rpc(String),
}
