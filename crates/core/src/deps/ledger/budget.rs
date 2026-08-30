//! The budget-source port: how much of an account is really settleable right now. The service asks
//! per `AccountKey` and stays ignorant of where the number comes from — a wallet's budget is a chain
//! read (`min(balanceOf, allowance→Aqua)`), a strategy virtual's is the registry snapshot. Read
//! just-in-time at hard-reserve, the "firm quote" confirmation before we commit to a fill.

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
