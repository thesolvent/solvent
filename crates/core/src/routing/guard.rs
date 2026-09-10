//! Lock-free routing visibility for strategies awaiting price restoration.

use std::collections::BTreeSet;
use std::sync::Arc;

use arc_swap::ArcSwap;

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
}

impl Default for StrategyGuard {
    fn default() -> Self {
        Self {
            guarded: ArcSwap::from_pointee(BTreeSet::new()),
        }
    }
}

impl StrategyGuard {
    /// Publish `key` as unavailable to subsequent routing snapshots.
    pub fn guard(&self, key: StrategyKey) {
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
}
