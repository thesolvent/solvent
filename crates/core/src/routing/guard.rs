//! Lock-free routing visibility for strategies awaiting price restoration.

use std::collections::BTreeSet;
use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::sync::{RwLock, RwLockReadGuard};

use crate::primitives::registry::StrategyKey;

/// An immutable view used for one candidate-selection pass.
#[derive(Debug, Clone, Default)]
pub struct GuardSnapshot(Arc<BTreeSet<StrategyKey>>);

impl GuardSnapshot {
    pub fn is_guarded(&self, key: &StrategyKey) -> bool {
        self.0.contains(key)
    }
}

/// Shared set of strategies withheld from new quotes while restoration is pending.
#[derive(Debug)]
pub struct StrategyGuard {
    guarded: ArcSwap<BTreeSet<StrategyKey>>,
    admission: RwLock<()>,
}

impl Default for StrategyGuard {
    fn default() -> Self {
        Self {
            guarded: ArcSwap::from_pointee(BTreeSet::new()),
            admission: RwLock::new(()),
        }
    }
}

impl StrategyGuard {
    /// Wait for reservations already at admission, then publish `key` as unavailable.
    pub async fn guard(&self, key: StrategyKey) {
        let _admission = self.admission.write().await;
        self.guarded.rcu(|current| {
            let mut next = BTreeSet::clone(current);
            next.insert(key);
            next
        });
    }

    /// Make `key` available to subsequent routing snapshots.
    pub fn unguard(&self, key: &StrategyKey) {
        self.guarded.rcu(|current| {
            let mut next = BTreeSet::clone(current);
            next.remove(key);
            next
        });
    }

    /// Freeze one coherent view for an entire route calculation.
    pub fn snapshot(&self) -> GuardSnapshot {
        GuardSnapshot(self.guarded.load_full())
    }

    pub fn is_guarded(&self, key: &StrategyKey) -> bool {
        self.guarded.load().contains(key)
    }

    /// Freeze guarded keys while a caller re-checks a plan and admits its reservation.
    pub(crate) async fn admission(&self) -> GuardAdmission<'_> {
        let barrier = self.admission.read().await;
        GuardAdmission {
            guarded: self.snapshot(),
            _barrier: barrier,
        }
    }
}

/// A coherent guard snapshot whose read permit prevents a guard from returning during admission.
pub(crate) struct GuardAdmission<'a> {
    guarded: GuardSnapshot,
    _barrier: RwLockReadGuard<'a, ()>,
}

impl GuardAdmission<'_> {
    pub(crate) fn is_guarded(&self, key: &StrategyKey) -> bool {
        self.guarded.is_guarded(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, B256};

    use crate::primitives::{MakerId, StrategyHash};

    fn key() -> StrategyKey {
        StrategyKey {
            maker: MakerId(Address::from([1; 20])),
            app: Address::from([2; 20]),
            strategy_hash: StrategyHash(B256::from([3; 32])),
        }
    }

    #[tokio::test]
    async fn guard_waits_for_admission_and_blocks_subsequent_admission() {
        let guards = Arc::new(StrategyGuard::default());
        let admission = guards.admission().await;
        let writer = {
            let guards = Arc::clone(&guards);
            tokio::spawn(async move { guards.guard(key()).await })
        };
        tokio::task::yield_now().await;
        assert!(!writer.is_finished());

        drop(admission);
        writer.await.unwrap();

        assert!(guards.admission().await.is_guarded(&key()));
    }
}
