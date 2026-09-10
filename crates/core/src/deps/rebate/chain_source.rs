//! Mined rebate executions emitted by the authorized filler.

use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::rebate::RebateExecutedEvent;

#[async_trait]
pub trait RebateChainSource: Send + Sync {
    async fn fetch(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<RebateExecutedEvent>, RebateChainSourceError>;
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RebateChainSourceError {
    #[error("rebate chain RPC: {0}")]
    Rpc(String),
    #[error("rebate execution event has no mined position")]
    Unpositioned,
}
