//! The derived liquidity snapshot and the pure `apply()` fold.
//!
//! `Snapshot` is a deterministic function of the ordered Aqua event stream:
//! replaying the same events always yields the same snapshot, so it can be
//! rebuilt from the log on restart. It keeps a dual index — strategies by their
//! identity key, and active two-token strategies by traded pair — so the router
//! can fetch every maker on a pair without scanning. The fold is curve-agnostic:
//! `Shipped` registers, `Pushed`/`Pulled` move balances, `Docked` tombstones.

use std::collections::{BTreeMap, BTreeSet};

use alloy_primitives::U256;

use super::event::{AquaEvent, EventCursor, StrategyKey};
use super::strategy::{MakerStrategy, TokenPair};

/// The event-sourced picture of all maker liquidity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Snapshot {
    strategies: BTreeMap<StrategyKey, MakerStrategy>,
    by_pair: BTreeMap<TokenPair, BTreeSet<StrategyKey>>,
    /// High-water mark: the position of the last event folded in. Guarantees
    /// exactly-once + strict ordering, so the reorg-overlap re-scan can safely
    /// re-feed already-seen events.
    applied_through: Option<EventCursor>,
}

impl Snapshot {
    /// The strategy for `key`, active or tombstoned.
    pub fn strategy(&self, key: &StrategyKey) -> Option<&MakerStrategy> {
        self.strategies.get(key)
    }

    /// The last event position folded in, or `None` for an empty snapshot. The
    /// sync loop aligns its durable cursor with this.
    pub fn applied_through(&self) -> Option<EventCursor> {
        self.applied_through
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

    /// Fold one event at its chain position `cursor`. Exactly-once + ordered: a
    /// `cursor` at or below the high-water mark is ignored (reorg-overlap re-feed
    /// or out-of-order), as are events for a strategy never seen `Shipped`. Reorg
    /// rewrites are handled a layer up, by rebuilding from the canonical log.
    pub fn apply(&mut self, cursor: EventCursor, event: AquaEvent) {
        if self.applied_through.is_some_and(|hw| cursor <= hw) {
            return;
        }
        self.applied_through = Some(cursor);
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
                    .or_insert_with(|| MakerStrategy::new(key, strategy));
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
                // (`push` to a docked slot reverts); ignore it if it appears.
                if let Some(s) = self.strategies.get_mut(&key) {
                    if s.active {
                        let before = s.pair();
                        let balance = s.balances.entry(token).or_insert(U256::ZERO);
                        *balance = balance.saturating_add(amount);
                        let after = s.pair();
                        Self::reindex(&mut self.by_pair, key, before, after);
                    }
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
                if let Some(s) = self.strategies.get_mut(&key) {
                    if s.active {
                        if let Some(balance) = s.balances.get_mut(&token) {
                            *balance = balance.saturating_sub(amount);
                        }
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

    fn cur(block: u64, log: u64) -> EventCursor {
        EventCursor {
            block_number: block,
            log_index: log,
        }
    }

    // Apply a sequence at strictly increasing cursors (one block each).
    fn apply_seq(snap: &mut Snapshot, events: impl IntoIterator<Item = AquaEvent>) {
        for (i, ev) in events.into_iter().enumerate() {
            snap.apply(cur(i as u64, 0), ev);
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
        assert_eq!(s.strategy, Bytes::from(vec![0u8]));
        assert_eq!(s.balance(&token(1)), U256::from(500u64));
    }

    #[test]
    fn push_or_pull_without_ship_is_ignored() {
        let mut snap = Snapshot::default();
        apply_seq(&mut snap, [pushed(0, 1, 100), pulled(0, 1, 50)]);
        assert!(snap.is_empty());
    }

    #[test]
    fn duplicate_and_out_of_order_events_are_ignored() {
        let mut snap = Snapshot::default();
        snap.apply(cur(1, 0), shipped(0));
        snap.apply(cur(1, 1), pushed(0, 1, 1000));
        snap.apply(cur(1, 2), pushed(0, 2, 1000));
        // Replay at the same cursor (reorg-overlap re-scan) -> no double-count.
        snap.apply(cur(1, 2), pushed(0, 2, 1000));
        // An older, out-of-order event -> ignored.
        snap.apply(cur(0, 5), pushed(0, 1, 5000));
        let s = snap.strategy(&key(0)).unwrap();
        assert_eq!(s.balance(&token(1)), U256::from(1000u64));
        assert_eq!(s.balance(&token(2)), U256::from(1000u64));
        assert_eq!(snap.applied_through(), Some(cur(1, 2)));
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig { failure_persistence: None, ..proptest::prelude::ProptestConfig::default() })]
        // For ANY ordered stream: re-feeding every event changes nothing
        // (exactly-once), and the dual index stays consistent with the map.
        #[test]
        fn fold_is_idempotent_and_index_consistent(
            ops in proptest::collection::vec(
                (0u8..4, 0u8..3, 0u8..3, 0u64..1_000_000),
                0..300,
            ),
        ) {
            let events: Vec<AquaEvent> = ops
                .into_iter()
                .map(|(op, s, t, amt)| match op {
                    0 => shipped(s),
                    1 => pushed(s, t, amt),
                    2 => pulled(s, t, amt),
                    _ => docked(s),
                })
                .collect();

            let mut once = Snapshot::default();
            let mut twice = Snapshot::default();
            for (i, ev) in events.iter().enumerate() {
                let c = cur(i as u64, 0);
                once.apply(c, ev.clone());
                twice.apply(c, ev.clone());
                twice.apply(c, ev.clone()); // immediate replay must be a no-op
            }
            proptest::prop_assert!(once == twice);

            // Forward: every indexed key is present, active, and matches its bucket.
            for (pair, set) in &once.by_pair {
                proptest::prop_assert!(!set.is_empty());
                for k in set {
                    let strat = once.strategies.get(k).expect("indexed key must exist");
                    proptest::prop_assert!(strat.active);
                    proptest::prop_assert_eq!(strat.pair(), Some(*pair));
                }
            }
            // Reverse: every active two-token strategy is indexed under its pair.
            for (k, strat) in &once.strategies {
                if strat.active {
                    if let Some(pair) = strat.pair() {
                        proptest::prop_assert!(
                            once.by_pair.get(&pair).is_some_and(|set| set.contains(k))
                        );
                    }
                }
            }
        }
    }
}
