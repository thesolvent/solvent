//! The budget-source port: an account's settleable budget, asked per `AccountKey` (wallet = chain
//! `min(balanceOf, allowance)`, strategy virtual = registry snapshot). A JIT firm confirm at reserve.

use std::collections::BTreeMap;

use alloy_primitives::U256;
use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::ledger::AccountKey;

#[async_trait]
pub trait BudgetSource: Send + Sync {
    /// The settleable budget for `account`.
    async fn budget(&self, account: &AccountKey) -> Result<U256, BudgetSourceError>;

    /// The budgets for many accounts at once, keyed by account — the sync path (the budget cache)
    /// reads every active maker each tick, so this is where an adapter batches its chain reads into
    /// one round-trip. The default is a sequential fallback for sources with nothing to batch (e.g.
    /// in-memory fakes).
    async fn budgets(
        &self,
        accounts: &[AccountKey],
    ) -> Result<BTreeMap<AccountKey, U256>, BudgetSourceError> {
        let mut out = BTreeMap::new();
        for account in accounts {
            out.insert(*account, self.budget(account).await?);
        }
        Ok(out)
    }
}

/// A budget-source failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BudgetSourceError {
    /// Reading the on-chain balance/allowance failed.
    #[error("chain read: {0}")]
    Chain(String),
}
