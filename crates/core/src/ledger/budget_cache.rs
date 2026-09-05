//! The budget cache: one synced, lock-free [`AvailableSnapshot`] of every active maker's pullable
//! wallet and strategy virtual, refreshed off the request path so a maker's chain reads happen once
//! per sync tick, batched — not once per request. The depth curve reads its caps from here with zero
//! RPC.
//!
//! DEFERRED — one snapshot for depth *and* the quote path (M2). The router today reads a *separate*
//! `LedgerService::available` snapshot (`budgets − reservations`), which is populated only at
//! reserve/recover — so it has no full-book sync yet. Converging the two is a `LedgerService`
//! `sync_budgets` that reuses this batched read + poller, letting both paths read one
//! net-of-reservations snapshot; it waits until the ledger goes live in the server. Until then this
//! cache carries no reservation holds — there are none in M1 — so its figures equal what the quote
//! path will see once wired.

use std::collections::BTreeSet;
use std::sync::Arc;

use alloy_primitives::U256;
use arc_swap::ArcSwap;

use crate::deps::ledger::BudgetSource;
use crate::ledger::AvailableSnapshot;
use crate::primitives::ledger::AccountKey;
use crate::primitives::registry::Snapshot;
use crate::registry::SharedSnapshot;
use crate::SolventError;

pub struct BudgetCache {
    source: Arc<dyn BudgetSource>,
    registry: Arc<SharedSnapshot>,
    available: ArcSwap<AvailableSnapshot>,
}

impl BudgetCache {
    pub fn new(source: Arc<dyn BudgetSource>, registry: Arc<SharedSnapshot>) -> Self {
        Self {
            source,
            registry,
            available: ArcSwap::from_pointee(AvailableSnapshot::default()),
        }
    }

    /// The whole synced snapshot, read lock-free — a caps handle for `select`, with no RPC.
    pub fn load(&self) -> Arc<AvailableSnapshot> {
        self.available.load_full()
    }

    /// Available room at one account, read lock-free.
    pub fn available(&self, account: &AccountKey) -> U256 {
        self.available.load().available(account)
    }

    /// Refresh every active maker's caps in one batched read, then publish atomically. On failure the
    /// last-good snapshot stays in place (the caller logs and retries next tick), so reads never see a
    /// torn or empty update once the cache has warmed.
    pub async fn refresh(&self) -> Result<(), SolventError> {
        let accounts = active_accounts(&self.registry.load());
        let budgets = self.source.budgets(&accounts).await?;
        self.available.store(Arc::new(AvailableSnapshot(budgets)));
        Ok(())
    }
}

/// The accounts to sync: each active strategy's wallet and virtual on both of its pair's tokens.
/// Wallets dedup per `(maker, token)` — a maker's strategies share one wallet.
fn active_accounts(snapshot: &Snapshot) -> Vec<AccountKey> {
    let mut walleted: BTreeSet<AccountKey> = BTreeSet::new();
    let mut accounts: Vec<AccountKey> = Vec::new();
    for strategy in snapshot.active_strategies() {
        let Some(pair) = strategy.pair() else {
            continue;
        };
        for token in [pair.lo, pair.hi] {
            let wallet = AccountKey::WalletBudget {
                maker: strategy.key.maker,
                token,
            };
            if walleted.insert(wallet) {
                accounts.push(wallet);
            }
            accounts.push(AccountKey::StrategyVirtual {
                maker: strategy.key.maker,
                strategy_hash: strategy.key.strategy_hash,
                token,
            });
        }
    }
    accounts
}
