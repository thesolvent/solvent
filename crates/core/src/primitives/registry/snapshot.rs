//! The derived liquidity snapshot and its pure `apply()` fold — a deterministic function of the
//! ordered Aqua event stream, rebuildable from the log on restart. A dual index (by key, by pair)
//! lets the router fetch a pair's makers without scanning.

use std::collections::{BTreeMap, BTreeSet};

use alloy_primitives::{Address, U256};
use itertools::Itertools;

use super::curve::{Curve, CurveSpec};
use super::event::{AquaEvent, StrategyKey};
use super::strategy::{MakerStrategy, TokenPair};

/// Per-asset activity derived from the snapshot: how many active strategies quote a token and in
/// which pairs. The source for the supported-asset list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActiveAsset {
    pub strategy_count: usize,
    pub pairs: BTreeSet<TokenPair>,
}

/// The three priceable curve shapes, without their parameters — for classifying a pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveKind {
    Xyc,
    Concentrated,
    Pegged,
}

/// How many active makers on a pair use each curve shape; `dominant` drives the pool `type`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StrategyCount {
    pub xyc: usize,
    pub concentrated: usize,
    pub pegged: usize,
}

impl StrategyCount {
    fn add(&mut self, curve: &Curve) {
        match curve {
            Curve::Xyc => self.xyc += 1,
            Curve::Concentrate { .. } => self.concentrated += 1,
            Curve::Pegged(_) => self.pegged += 1,
        }
    }

    /// The most-common curve shape (ties broken Xyc > Concentrated > Pegged); `None` if empty.
    pub fn dominant(&self) -> Option<CurveKind> {
        [
            (CurveKind::Xyc, self.xyc),
            (CurveKind::Concentrated, self.concentrated),
            (CurveKind::Pegged, self.pegged),
        ]
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .max_by_key(|(_, n)| *n)
        .map(|(kind, _)| kind)
    }
}

/// A pool = the aggregation of all active, priceable strategies over one pair.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PoolStats {
    pub maker_count: usize,
    pub min_fee_bps: u32,
    pub max_fee_bps: u32,
    pub popular_fee_bps: u32,
    pub curve_mix: StrategyCount,
}

/// A strategy's total flat fee in real bps. SwapVM `bps` are at `1e9` = 100%, so one real bp
/// (`0.01%`) is `1e5` SwapVM units.
fn fee_in_bps(fees_in_bps: &[u32]) -> u32 {
    let swapvm: u64 = fees_in_bps.iter().map(|f| *f as u64).sum();
    (swapvm / 100_000) as u32
}

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

    /// Every token quoted by an active, pair-routable strategy → its active-strategy count and the
    /// pairs it trades. Each active strategy contributes to both of its pair's tokens.
    pub fn active_assets(&self) -> BTreeMap<Address, ActiveAsset> {
        self.strategies
            .values()
            .filter(|s| s.active)
            .filter_map(|s| s.pair())
            .flat_map(|pair| [pair.lo, pair.hi].map(move |token| (token, pair)))
            .into_grouping_map()
            .fold(ActiveAsset::default(), |mut asset, _token, pair| {
                asset.strategy_count += 1;
                asset.pairs.insert(pair);
                asset
            })
            .into_iter()
            .collect()
    }

    /// Aggregate active, priceable strategies into per-pair pool stats: maker count, fee range,
    /// most-common fee, and the curve mix. `Unsupported` strategies (unpriceable) are excluded.
    pub fn pool_stats(&self) -> BTreeMap<TokenPair, PoolStats> {
        self.strategies
            .values()
            .filter(|s| s.active)
            .filter_map(|s| match &s.curve {
                CurveSpec::Priceable { curve, fees_in_bps } => s
                    .pair()
                    .map(|pair| (pair, (*curve, fee_in_bps(fees_in_bps)))),
                CurveSpec::Unsupported => None,
            })
            .into_grouping_map()
            .fold(
                (StrategyCount::default(), BTreeMap::<u32, usize>::new()),
                |(mut mix, mut fees), _pair, (curve, fee_bps)| {
                    mix.add(&curve);
                    *fees.entry(fee_bps).or_default() += 1;
                    (mix, fees)
                },
            )
            .into_iter()
            .map(|(pair, (curve_mix, fees))| {
                let stats = PoolStats {
                    maker_count: fees.values().sum(),
                    min_fee_bps: fees.keys().next().copied().unwrap_or(0),
                    max_fee_bps: fees.keys().next_back().copied().unwrap_or(0),
                    popular_fee_bps: fees
                        .iter()
                        .max_by_key(|(_, count)| **count)
                        .map(|(fee, _)| *fee)
                        .unwrap_or(0),
                    curve_mix,
                };
                (pair, stats)
            })
            .collect()
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
    fn active_assets_counts_active_strategies_and_pairs_per_token() {
        let mut snap = Snapshot::default();
        apply_seq(
            &mut snap,
            [
                shipped(0),
                pushed(0, 1, 100),
                pushed(0, 2, 100), // active pair (1,2)
                shipped(1),
                pushed(1, 2, 100),
                pushed(1, 3, 100), // active pair (2,3), shares token 2
                shipped(2),
                pushed(2, 1, 100),
                pushed(2, 2, 100),
                docked(2), // docked pair (1,2) — excluded
            ],
        );

        let assets = snap.active_assets();
        let pair12 = TokenPair::new(token(1), token(2));
        let pair23 = TokenPair::new(token(2), token(3));

        assert_eq!(assets.len(), 3);
        assert_eq!(assets[&token(1)].strategy_count, 1); // docked strategy 2 not counted
        assert_eq!(assets[&token(2)].strategy_count, 2);
        assert_eq!(assets[&token(3)].strategy_count, 1);
        assert!(assets[&token(2)].pairs.contains(&pair12));
        assert!(assets[&token(2)].pairs.contains(&pair23));
        assert_eq!(assets[&token(1)].pairs.len(), 1);
    }

    #[test]
    fn pool_stats_aggregates_active_priceable_strategies() {
        fn priceable(
            s: u8,
            a: Address,
            b: Address,
            curve: Curve,
            fee_swapvm: u32,
        ) -> MakerStrategy {
            let mut st = MakerStrategy::new(key(s), &[]);
            st.curve = CurveSpec::Priceable {
                curve,
                fees_in_bps: vec![fee_swapvm],
            };
            st.balances.insert(a, U256::from(1u64));
            st.balances.insert(b, U256::from(1u64));
            st
        }
        let conc = Curve::Concentrate {
            sqrt_price_min: U256::from(1u64),
            sqrt_price_max: U256::from(2u64),
        };
        let mut docked = priceable(9, token(1), token(2), Curve::Xyc, 900_000);
        docked.active = false;
        let unsupported = {
            let mut st = MakerStrategy::new(key(8), &[]); // empty program → Unsupported
            st.balances.insert(token(1), U256::from(1u64));
            st.balances.insert(token(2), U256::from(1u64));
            st
        };
        let snap = Snapshot::from_strategies([
            priceable(0, token(1), token(2), Curve::Xyc, 500_000), // 5 bps
            priceable(1, token(1), token(2), Curve::Xyc, 500_000), // 5 bps
            priceable(2, token(1), token(2), Curve::Xyc, 100_000), // 1 bps
            priceable(3, token(2), token(3), conc, 3_000_000),     // 30 bps concentrated
            docked,
            unsupported,
        ]);

        let stats = snap.pool_stats();
        let p12 = &stats[&TokenPair::new(token(1), token(2))];
        assert_eq!(p12.maker_count, 3); // docked + unsupported excluded
        assert_eq!(p12.min_fee_bps, 1);
        assert_eq!(p12.max_fee_bps, 5);
        assert_eq!(p12.popular_fee_bps, 5); // two makers at 5 bps
        assert_eq!(p12.curve_mix.dominant(), Some(CurveKind::Xyc));

        let p23 = &stats[&TokenPair::new(token(2), token(3))];
        assert_eq!(p23.maker_count, 1);
        assert_eq!(p23.max_fee_bps, 30);
        assert_eq!(p23.curve_mix.dominant(), Some(CurveKind::Concentrated));
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
