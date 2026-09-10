//! Durable storage for open rebate batches and their executable payloads.

use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::rebate::{RebateBatch, RebateSettlement};
use crate::primitives::{ChainId, RebateBatchId, StrategyHash};

#[async_trait]
pub trait RebateStore: Send + Sync {
    /// Atomically persist the complete open batch. Existing accrual identities must retain the
    /// exact same body so a replay cannot silently rewrite maker attribution.
    async fn save(&self, batch: &RebateBatch) -> Result<(), RebateStoreError>;

    /// Every still-open batch required to rebuild guards and executable work after restart.
    async fn open_batches(&self) -> Result<Vec<RebateBatch>, RebateStoreError>;

    /// Close a batch without erasing its accrual history.
    async fn close(&self, id: RebateBatchId) -> Result<(), RebateStoreError>;

    /// Last block fully scanned for filler rebate executions on this chain.
    async fn scan_cursor(&self, chain: ChainId) -> Result<Option<u64>, RebateStoreError>;

    /// Advance the execution-log cursor after every event through this block is reconciled.
    async fn save_scan_cursor(
        &self,
        chain: ChainId,
        block_number: u64,
    ) -> Result<(), RebateStoreError>;

    /// Persist an execution before consuming its inventory reservation.
    async fn begin_settlement(&self, settlement: &RebateSettlement)
        -> Result<(), RebateStoreError>;

    /// Settlements interrupted after their on-chain event was observed.
    async fn pending_settlements(&self) -> Result<Vec<RebateSettlement>, RebateStoreError>;

    /// Atomically retain executed history and close its open batch.
    async fn finish_settlement(&self, id: RebateBatchId) -> Result<(), RebateStoreError>;
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RebateStoreError {
    #[error("open rebate batch conflicts for strategy {0}")]
    Conflict(StrategyHash),
    #[error("rebate database: {0}")]
    Db(String),
}
