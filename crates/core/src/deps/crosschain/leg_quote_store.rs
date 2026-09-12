use alloy_primitives::B256;
use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::crosschain::LegQuote;

#[derive(Debug, Error)]
pub enum LegQuoteStoreError {
    #[error("cross-chain quote store backend: {0}")]
    Backend(String),
    #[error("cross-chain quote fingerprint conflicts with stored terms")]
    Conflict,
}

#[async_trait]
pub trait LegQuoteStore: Send + Sync {
    async fn insert(&self, quote: &LegQuote) -> Result<LegQuote, LegQuoteStoreError>;
    async fn load(&self, quote_id: B256) -> Result<Option<LegQuote>, LegQuoteStoreError>;
}
