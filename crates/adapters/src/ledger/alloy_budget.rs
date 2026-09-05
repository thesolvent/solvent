//! The chain-reading [`BudgetSource`]: a wallet's budget is `min(balanceOf, allowance→Aqua)` via one
//! Multicall3; a strategy virtual's is the registry snapshot.

use std::collections::BTreeMap;
use std::sync::Arc;

use alloy::{
    primitives::{Address, U256},
    providers::Provider,
};
use async_trait::async_trait;
use solvent_core::{
    deps::ledger::{BudgetSource, BudgetSourceError},
    primitives::{ledger::AccountKey, registry::StrategyKey, MakerId, StrategyHash},
    registry::SharedSnapshot,
};

use crate::erc20::{read_balance_allowances, IERC20};

/// A wallet account reduced to the two addresses its reads need: the maker (balance owner) and the
/// token contract.
type Wallet = (AccountKey, MakerId, Address);

pub struct AlloyBudgetSource<P> {
    provider: P,
    aqua: Address,
    app: Address,
    registry: Arc<SharedSnapshot>,
}

impl<P: Provider + Clone + 'static> AlloyBudgetSource<P> {
    /// `aqua` is the allowance spender; `app` is the SwapVM router that keys strategies in the
    /// snapshot; `registry` is the live liquidity snapshot the strategy-virtual budget reads.
    pub fn new(provider: P, aqua: Address, app: Address, registry: Arc<SharedSnapshot>) -> Self {
        Self {
            provider,
            aqua,
            app,
            registry,
        }
    }
    /// A strategy virtual budget: the active strategy's registry balance in `token`. A docked
    /// strategy keeps a tombstone balance but can't be pulled, so it reads zero.
    fn virtual_budget(&self, maker: MakerId, strategy_hash: StrategyHash, token: &Address) -> U256 {
        let key = StrategyKey {
            maker,
            app: self.app,
            strategy_hash,
        };
        self.registry
            .load()
            .strategy(&key)
            .filter(|strategy| strategy.active)
            .map(|strategy| strategy.balance(token))
            .unwrap_or(U256::ZERO)
    }

    /// The pullable `min(balanceOf, allowance→Aqua)` for every wallet, batched into two Multicall3
    /// aggregates — two round-trips for the whole book, not two per maker.
    async fn wallet_pullables(
        &self,
        wallets: &[Wallet],
    ) -> Result<BTreeMap<AccountKey, U256>, BudgetSourceError> {
        let accounts: Vec<(Address, Address)> = wallets
            .iter()
            .map(|(_, maker, token)| (maker.0, *token))
            .collect();
        let reads = read_balance_allowances(&self.provider, self.aqua, &accounts)
            .await
            .map_err(chain)?;
        Ok(wallets
            .iter()
            .zip(reads)
            .map(|((account, _, _), read)| (*account, read.balance.min(read.allowance)))
            .collect())
    }
}

fn chain(e: impl std::fmt::Display) -> BudgetSourceError {
    BudgetSourceError::Chain(e.to_string())
}

#[async_trait]
impl<P: Provider + Clone + 'static> BudgetSource for AlloyBudgetSource<P> {
    async fn budget(&self, account: &AccountKey) -> Result<U256, BudgetSourceError> {
        match account {
            AccountKey::WalletBudget { maker, token } => {
                // One atomic same-block read: balanceOf and allowance from a single aggregate — the
                // firm confirm at reserve wants both from the same block.
                let erc20 = IERC20::new(*token, self.provider.clone());
                let (balance, allowance) = self
                    .provider
                    .multicall()
                    .add(erc20.balanceOf(maker.0))
                    .add(erc20.allowance(maker.0, self.aqua))
                    .aggregate()
                    .await
                    .map_err(chain)?;
                Ok(balance.min(allowance))
            }
            AccountKey::StrategyVirtual {
                maker,
                strategy_hash,
                token,
            } => Ok(self.virtual_budget(*maker, *strategy_hash, token)),
        }
    }

    async fn budgets(
        &self,
        accounts: &[AccountKey],
    ) -> Result<BTreeMap<AccountKey, U256>, BudgetSourceError> {
        let wallets: Vec<Wallet> = accounts
            .iter()
            .filter_map(|account| match account {
                AccountKey::WalletBudget { maker, token } => Some((*account, *maker, *token)),
                AccountKey::StrategyVirtual { .. } => None,
            })
            .collect();
        let mut budgets = self.wallet_pullables(&wallets).await?;
        for account in accounts {
            if let AccountKey::StrategyVirtual {
                maker,
                strategy_hash,
                token,
            } = account
            {
                budgets.insert(*account, self.virtual_budget(*maker, *strategy_hash, token));
            }
        }
        Ok(budgets)
    }
}
