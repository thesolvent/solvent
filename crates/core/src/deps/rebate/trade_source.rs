//! Confirmed strategy fills waiting to enter the rebate state machine.

use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::rebate::RebateAccrual;
use crate::primitives::registry::StrategyKey;

#[async_trait]
pub trait RebateAccrualSource: Send + Sync {
    /// Confirmed legs absent from durable rebate accrual history, oldest first.
    async fn pending(&self, limit: u32) -> Result<Vec<RebateAccrual>, RebateAccrualSourceError>;

    /// Whether an earlier submitted fill can still change this strategy's balance.
    async fn has_unsettled(&self, strategy: &StrategyKey)
        -> Result<bool, RebateAccrualSourceError>;
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RebateAccrualSourceError {
    #[error("rebate trade source: {0}")]
    Db(String),
}
