//! Durable storage for open rebate batches and their executable payloads.

use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::rebate::RebateBatch;
use crate::primitives::{RebateBatchId, StrategyHash};

#[async_trait]
pub trait RebateStore: Send + Sync {
    /// Atomically persist the complete open batch. Existing accrual identities must retain the
    /// exact same body so a replay cannot silently rewrite maker attribution.
    async fn save(&self, batch: &RebateBatch) -> Result<(), RebateStoreError>;

    /// Every still-open batch required to rebuild guards and executable work after restart.
    async fn open_batches(&self) -> Result<Vec<RebateBatch>, RebateStoreError>;

    /// Close a batch without erasing its accrual history.
    async fn close(&self, id: RebateBatchId) -> Result<(), RebateStoreError>;
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RebateStoreError {
    #[error("open rebate batch conflicts for strategy {0}")]
    Conflict(StrategyHash),
    #[error("rebate database: {0}")]
    Db(String),
}
