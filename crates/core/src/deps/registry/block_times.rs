//! Block timestamps for strategy history, keyed by hash so reorganized heights cannot reuse a time.

use alloy_primitives::B256;
use async_trait::async_trait;
use std::collections::BTreeMap;
use thiserror::Error;

#[async_trait]
pub trait BlockTimes: Send + Sync {
    async fn timestamps(&self, hashes: &[B256]) -> Result<BTreeMap<B256, u64>, BlockTimesError>;
}

#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum BlockTimesError {
    #[error("block timestamp RPC: {0}")]
    Rpc(String),
    #[error("block {0} is unavailable")]
    Missing(B256),
}
