use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::crosschain::{LegQuote, LegQuoteRequest};

#[derive(Debug, Error)]
pub enum LegQuoterError {
    #[error("no route for this pair and size")]
    NoRoute,
    #[error("chain-local quote backend: {0}")]
    Backend(String),
}

#[async_trait]
pub trait LegQuoter: Send + Sync {
    async fn quote(&self, request: &LegQuoteRequest) -> Result<LegQuote, LegQuoterError>;
}
