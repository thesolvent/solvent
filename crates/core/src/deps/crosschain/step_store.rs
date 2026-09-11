use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::crosschain::{ChainExecutionPlan, PreparedStep, RemoteCommand};
use crate::primitives::{AggregateQuoteId, CrossChainOrderId};

#[derive(Debug, Error)]
pub enum StepStoreError {
    #[error("cross-chain step store backend: {0}")]
    Backend(String),
    #[error("cross-chain execution plan conflicts with existing staged calls")]
    Conflict,
}

#[async_trait]
pub trait StepStore: Send + Sync {
    async fn stage(
        &self,
        order_id: CrossChainOrderId,
        plan: &ChainExecutionPlan,
    ) -> Result<(), StepStoreError>;
    async fn load(
        &self,
        order_id: CrossChainOrderId,
        command: RemoteCommand,
    ) -> Result<Option<PreparedStep>, StepStoreError>;
    async fn aggregate_id(
        &self,
        order_id: CrossChainOrderId,
        command: RemoteCommand,
    ) -> Result<Option<AggregateQuoteId>, StepStoreError>;
}
