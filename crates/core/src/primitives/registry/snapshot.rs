//! The derived liquidity snapshot and its pure `apply()` fold — a deterministic function of the
//! ordered Aqua event stream, rebuildable from the log on restart. A dual index (by key, by pair)
//! lets the router fetch a pair's makers without scanning.

use std::collections::{BTreeMap, BTreeSet};

use alloy_primitives::U256;

use super::event::{AquaEvent, StrategyKey};
use super::strategy::{MakerStrategy, TokenPair};

/// The event-sourced picture of all maker liquidity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Snapshot {
    strategies: BTreeMap<StrategyKey, MakerStrategy>,
    by_pair: BTreeMap<TokenPair, BTreeSet<StrategyKey>>,
}

impl Snapshot {
    /// The strategy for `key`, active or tombstoned.
    pub fn strategy(&self, key: &StrategyKey) -> Option<&MakerStrategy> {
        self.strategies.get(key)
    }

    /// Total strategies tracked (including tombstones).
    pub fn len(&self) -> usize {
        self.strategies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strategies.is_empty()
    }

    /// Active strategies quoting `pair` — the router's candidate funnel.
    pub fn active_strategies_for_pair(
        &self,
        pair: TokenPair,
    ) -> impl Iterator<Item = &MakerStrategy> {
        self.by_pair
            .get(&pair)
            .into_iter()
            .flatten()
            .filter_map(move |key| self.strategies.get(key).filter(|s| s.active))
    }

    /// Fold one event into the snapshot — a pure step: `Shipped` registers,
    /// `Pushed`/`Pulled` move balances, `Docked` tombstones; events for a strategy
    /// never seen `Shipped` are ignored. Dedup and chain-ordering are the caller's
    /// job (the sync loop applies each event exactly once, in order).
    pub fn apply(&mut self, event: AquaEvent) {
        match event {
            AquaEvent::Shipped {
                maker,
                app,
                strategy_hash,
                strategy,
            } => {
                let key = StrategyKey {
                    maker,
                    app,
                    strategy_hash,
                };
                // Strategies are immutable: a repeat `Shipped` for a live key is a
                // no-op (on-chain `ship` reverts on a non-empty slot).
                self.strategies
                    .entry(key)
                    .or_insert_with(|| MakerStrategy::new(key, &strategy));
            }
            AquaEvent::Pushed {
                maker,
                app,
                strategy_hash,
                token,
                amount,
            } => {
                let key = StrategyKey {
                    maker,
                    app,
                    strategy_hash,
                };
                // A `Pushed` to a tombstoned strategy is impossible on-chain
                // (`push` to a docked slot reverts); the `filter` ignores it.
                if let Some(s) = self.strategies.get_mut(&key).filter(|s| s.active) {
                    let before = s.pair();
                    let balance = s.balances.entry(token).or_insert(U256::ZERO);
                    *balance = balance.saturating_add(amount);
                    let after = s.pair();
                    Self::reindex(&mut self.by_pair, key, before, after);
                }
            }
            AquaEvent::Pulled {
                maker,
                app,
                strategy_hash,
                token,
                amount,
            } => {
                let key = StrategyKey {
                    maker,
                    app,
                    strategy_hash,
                };
                if let Some(s) = self.strategies.get_mut(&key).filter(|s| s.active) {
                    if let Some(balance) = s.balances.get_mut(&token) {
                        *balance = balance.saturating_sub(amount);
                    }
                }
            }
            AquaEvent::Docked {
                maker,
                app,
                strategy_hash,
            } => {
                let key = StrategyKey {
                    maker,
                    app,
                    strategy_hash,
                };
                if let Some(s) = self.strategies.get_mut(&key) {
                    s.active = false;
                    let pair = s.pair();
                    Self::reindex(&mut self.by_pair, key, pair, None);
                }
            }
        }
    }

    /// Move `key` from its `old` pair bucket to its `new` one (either may be
    /// `None`). Handles first-token, pair-complete, token-count-change, and
    /// tombstone transitions uniformly; empties are pruned.
    fn reindex(
        by_pair: &mut BTreeMap<TokenPair, BTreeSet<StrategyKey>>,
        key: StrategyKey,
        old: Option<TokenPair>,
        new: Option<TokenPair>,
    ) {
        if old == new {
            return;
        }
        if let Some(pair) = old {
            if let Some(set) = by_pair.get_mut(&pair) {
                set.remove(&key);
                if set.is_empty() {
                    by_pair.remove(&pair);
                }
            }
        }
        if let Some(pair) = new {
            by_pair.entry(pair).or_default().insert(key);
        }
    }
}

#[cfg(test)]
impl Snapshot {
    /// Build directly from strategies, bypassing the event fold — for tests in other
    /// slices (e.g. routing) that need a populated snapshot without shipping programs.
    pub(crate) fn from_strategies(strategies: impl IntoIterator<Item = MakerStrategy>) -> Self {
        let mut snapshot = Snapshot::default();
        for strategy in strategies {
            if let Some(pair) = strategy.pair() {
                snapshot
                    .by_pair
                    .entry(pair)
                    .or_default()
                    .insert(strategy.key);
            }
            snapshot.strategies.insert(strategy.key, strategy);
        }
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{MakerId, StrategyHash};
    use alloy_primitives::{Address, Bytes, B256};

    fn maker(n: u8) -> MakerId {
        MakerId(Address::from([n; 20]))
    }
    fn app() -> Address {
        Address::from([0xAA; 20])
    }
    fn hash(n: u8) -> StrategyHash {
        StrategyHash(B256::from([n; 32]))
    }
    fn token(n: u8) -> Address {
        Address::from([n; 20])
    }
    fn key(s: u8) -> StrategyKey {
        StrategyKey {
            maker: maker(s),
            app: app(),
            strategy_hash: hash(s),
        }
    }

    fn shipped(s: u8) -> AquaEvent {
        AquaEvent::Shipped {
            maker: maker(s),
            app: app(),
            strategy_hash: hash(s),
            strategy: Bytes::from(vec![s]),
        }
    }
    fn pushed(s: u8, t: u8, amt: u64) -> AquaEvent {
        AquaEvent::Pushed {
            maker: maker(s),
            app: app(),
            strategy_hash: hash(s),
            token: token(t),
            amount: U256::from(amt),
        }
    }
    fn pulled(s: u8, t: u8, amt: u64) -> AquaEvent {
        AquaEvent::Pulled {
            maker: maker(s),
            app: app(),
            strategy_hash: hash(s),
            token: token(t),
            amount: U256::from(amt),
        }
    }
    fn docked(s: u8) -> AquaEvent {
        AquaEvent::Docked {
            maker: maker(s),
            app: app(),
            strategy_hash: hash(s),
        }
    }

    fn apply_seq(snap: &mut Snapshot, events: impl IntoIterator<Item = AquaEvent>) {
        for ev in events {
            snap.apply(ev);
        }
    }

    #[test]
    fn ship_then_swap_tracks_balances_and_indexes_pair() {
        let mut snap = Snapshot::default();
        apply_seq(
            &mut snap,
            [
                shipped(0),
                pushed(0, 1, 1000),
                pushed(0, 2, 1000),
                pushed(0, 1, 100),
                pulled(0, 2, 90),
            ],
        );
        let s = snap.strategy(&key(0)).unwrap();
        assert_eq!(s.balance(&token(1)), U256::from(1100u64));
        assert_eq!(s.balance(&token(2)), U256::from(910u64));
        assert!(s.active);

        let pair = TokenPair::new(token(1), token(2));
        let found: Vec<_> = snap
            .active_strategies_for_pair(pair)
            .map(|s| s.key)
            .collect();
        assert_eq!(found, vec![key(0)]);
    }

    #[test]
    fn dock_tombstones_and_deindexes() {
        let mut snap = Snapshot::default();
        apply_seq(
            &mut snap,
            [
                shipped(0),
                pushed(0, 1, 1000),
                pushed(0, 2, 1000),
                docked(0),
            ],
        );
        let s = snap.strategy(&key(0)).unwrap();
        assert!(!s.active);
        // Balances retained as a tombstone, but no longer routable.
        assert_eq!(s.balance(&token(1)), U256::from(1000u64));
        let pair = TokenPair::new(token(1), token(2));
        assert_eq!(snap.active_strategies_for_pair(pair).count(), 0);
    }

    #[test]
    fn reship_is_immutable() {
        let mut snap = Snapshot::default();
        // A second Shipped for the same key must not reset program or balances.
        apply_seq(
            &mut snap,
            [
                shipped(0),
                pushed(0, 1, 500),
                AquaEvent::Shipped {
                    maker: maker(0),
                    app: app(),
                    strategy_hash: hash(0),
                    strategy: Bytes::from(vec![0xFF]),
                },
            ],
        );
        let s = snap.strategy(&key(0)).unwrap();
        assert_eq!(s.balance(&token(1)), U256::from(500u64));
    }

    #[test]
    fn push_or_pull_without_ship_is_ignored() {
        let mut snap = Snapshot::default();
        apply_seq(&mut snap, [pushed(0, 1, 100), pulled(0, 1, 50)]);
        assert!(snap.is_empty());
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig { failure_persistence: None, ..proptest::prelude::ProptestConfig::default() })]
        // For ANY event stream, the dual index stays consistent with the map:
        // every indexed key is an active strategy on that pair, and vice versa.
        #[test]
        fn fold_keeps_dual_index_consistent(
            ops in proptest::collection::vec(
                (0u8..4, 0u8..3, 0u8..3, 0u64..1_000_000),
                0..300,
            ),
        ) {
            let mut snap = Snapshot::default();
            for (op, s, t, amt) in ops {
                snap.apply(match op {
                    0 => shipped(s),
                    1 => pushed(s, t, amt),
                    2 => pulled(s, t, amt),
                    _ => docked(s),
                });
            }

            // Forward: every indexed key is present, active, and matches its bucket.
            for (pair, set) in &snap.by_pair {
                proptest::prop_assert!(!set.is_empty());
                for k in set {
                    let strat = snap.strategies.get(k).expect("indexed key must exist");
                    proptest::prop_assert!(strat.active);
                    proptest::prop_assert_eq!(strat.pair(), Some(*pair));
                }
            }
            // Reverse: every active two-token strategy is indexed under its pair.
            for (k, strat) in &snap.strategies {
                if strat.active {
                    if let Some(pair) = strat.pair() {
                        proptest::prop_assert!(
                            snap.by_pair.get(&pair).is_some_and(|set| set.contains(k))
                        );
                    }
                }
            }
        }
    }
}
