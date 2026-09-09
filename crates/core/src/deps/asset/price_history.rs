//! Historical pair-price source. This read path is separate from the live oracle used by routing.

use alloy_primitives::Address;
use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::asset::{PairPriceHistory, PriceHistoryPeriod};

#[async_trait]
pub trait PairPriceHistorySource: Send + Sync {
    async fn history(
        &self,
        base: Address,
        quote: Address,
        period: PriceHistoryPeriod,
    ) -> Result<PairPriceHistory, PairPriceHistorySourceError>;
}

#[derive(Clone, Debug, Error)]
#[non_exhaustive]
pub enum PairPriceHistorySourceError {
    #[error("no historical price source for token `{0}`")]
    NotFound(Address),
    #[error("historical price source: {0}")]
    Source(String),
    #[error("invalid historical price data: {0}")]
    InvalidData(String),
}
