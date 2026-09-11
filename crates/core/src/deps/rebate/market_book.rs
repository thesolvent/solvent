//! Executable bid/ask prices with bounded age for conservative restoration valuation.

use alloy_primitives::Address;
use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::rebate::RebateMarket;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebateMarketRequest {
    pub token_a: Address,
    pub token_a_decimals: u8,
    pub token_b: Address,
    pub token_b_decimals: u8,
    pub max_age_secs: u64,
}

impl RebateMarketRequest {
    pub fn new(
        token_a: Address,
        token_a_decimals: u8,
        token_b: Address,
        token_b_decimals: u8,
        max_age_secs: u64,
    ) -> Self {
        Self {
            token_a,
            token_a_decimals,
            token_b,
            token_b_decimals,
            max_age_secs,
        }
    }
}

#[async_trait]
pub trait RebateMarketBook: Send + Sync {
    async fn market(
        &self,
        request: RebateMarketRequest,
    ) -> Result<RebateMarket, RebateMarketBookError>;
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RebateMarketBookError {
    #[error("no executable market book for token {0}")]
    NotFound(Address),
    #[error("market book for token {0} is stale")]
    Stale(Address),
    #[error("market book is invalid: {0}")]
    Invalid(&'static str),
}
