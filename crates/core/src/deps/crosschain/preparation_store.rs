use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::crosschain::LegRole;
use crate::primitives::crosschain::Preparation;
use crate::primitives::{AggregateQuoteId, PrepareToken};

#[derive(Debug, Error)]
pub enum PreparationStoreError {
    #[error("preparation store backend: {0}")]
    Backend(String),
    #[error("preparation token conflicts with an existing hold")]
    Conflict,
}

#[async_trait]
pub trait PreparationStore: Send + Sync {
    async fn insert(&self, preparation: &Preparation)
        -> Result<Preparation, PreparationStoreError>;
    async fn load(&self, token: PrepareToken)
        -> Result<Option<Preparation>, PreparationStoreError>;
    async fn load_for_quote(
        &self,
        quote_id: AggregateQuoteId,
        role: LegRole,
    ) -> Result<Option<Preparation>, PreparationStoreError>;
    async fn update(&self, preparation: &Preparation) -> Result<(), PreparationStoreError>;
}
