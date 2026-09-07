//! The balances-oracle port: a wallet's on-chain balance and pullable amount per token. Distinct
//! from [`BudgetSource`](crate::deps::ledger::BudgetSource), which collapses to the pullable `min` —
//! the UI needs both figures.

use std::collections::BTreeMap;

use alloy_primitives::Address;
use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::amount::Holdings;

#[async_trait]
pub trait BalancesOracle: Send + Sync {
    /// `(balance, pullable)` for `owner` across `tokens`, keyed by token address. A token the owner
    /// has never touched reads as zero. `pullable = min(balance, allowance→settlement)`.
    async fn holdings(
        &self,
        owner: Address,
        tokens: &[Address],
    ) -> Result<BTreeMap<Address, Holdings>, BalancesOracleError>;
}

/// A balances-oracle failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BalancesOracleError {
    /// Reading the on-chain balances/allowances failed.
    #[error("chain read: {0}")]
    Chain(String),
}
