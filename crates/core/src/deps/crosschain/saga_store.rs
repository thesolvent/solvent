use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::crosschain::CrossChainSaga;
use crate::primitives::CrossChainOrderId;

#[derive(Debug, Error)]
pub enum SagaStoreError {
    #[error("cross-chain saga store backend: {0}")]
    Backend(String),
    #[error("cross-chain order conflicts with an existing saga")]
    Conflict,
}

#[async_trait]
pub trait SagaStore: Send + Sync {
    async fn insert(&self, saga: &CrossChainSaga) -> Result<CrossChainSaga, SagaStoreError>;
    async fn load(
        &self,
        order_id: CrossChainOrderId,
    ) -> Result<Option<CrossChainSaga>, SagaStoreError>;
    async fn update(&self, saga: &CrossChainSaga) -> Result<(), SagaStoreError>;
    async fn recoverable(&self) -> Result<Vec<CrossChainSaga>, SagaStoreError>;
}
