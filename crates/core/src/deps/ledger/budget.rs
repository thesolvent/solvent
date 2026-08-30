//! The budget-source port: an account's settleable budget, asked per `AccountKey` (wallet = chain
//! `min(balanceOf, allowance)`, strategy virtual = registry snapshot). A JIT firm confirm at reserve.

use alloy_primitives::U256;
use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::ledger::AccountKey;

#[async_trait]
pub trait BudgetSource: Send + Sync {
    /// The settleable budget for `account`.
    async fn budget(&self, account: &AccountKey) -> Result<U256, BudgetSourceError>;
}

/// A budget-source failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BudgetSourceError {
    /// Reading the on-chain balance/allowance failed.
    #[error("chain read: {0}")]
    Chain(String),
}
