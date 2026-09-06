//! The maker read-surface: turns shipped strategies into positions, the positions list, and the
//! roster — composing the registry snapshot, the balances oracle (pullable), USD valuation, the T1
//! range decoder, and the T2 metrics store. On-chain history (since/docked block, opening balances,
//! fills, mid-price) is filled by a later task.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use alloy_primitives::{Address, U256};
use itertools::Itertools;

use crate::asset::AssetManager;
use crate::deps::balances::BalancesOracle;
use crate::deps::ledger::Clock;
use crate::deps::maker_metrics::{MakerMetricsStore, PositionMetrics};
use crate::primitives::amount::{TokenAmount, TokenAmounts};
use crate::primitives::maker::{
    ActiveStats, Economics, MakerSummary, Position, PositionBalances, PriceRange, Split,
};
use crate::primitives::pool::classify_pair;
use crate::primitives::registry::{
    curve_label, fee_in_bps, price_range, Curve, CurveKind, CurveSpec, MakerStrategy,
    PositionRange, RangeKind, TokenPair,
};
use crate::primitives::{MakerId, StrategyHash, Usd};
use crate::registry::SharedSnapshot;
use crate::valuation::Valuation;
use crate::SolventError;

const DAY: u64 = 86_400;
/// The activity window for economics and active-stats.
const WINDOW_DAYS: u64 = 7;

pub struct MakerService {
    registry: Arc<SharedSnapshot>,
    assets: Arc<AssetManager>,
    valuation: Arc<Valuation>,
    metrics: Arc<dyn MakerMetricsStore>,
    balances: Arc<dyn BalancesOracle>,
    clock: Arc<dyn Clock>,
}

impl MakerService {
    pub fn new(
        registry: Arc<SharedSnapshot>,
        assets: Arc<AssetManager>,
        valuation: Arc<Valuation>,
        metrics: Arc<dyn MakerMetricsStore>,
        balances: Arc<dyn BalancesOracle>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            registry,
            assets,
            valuation,
            metrics,
            balances,
            clock,
        }
    }

    /// One position by its strategy hash (active or docked), with detail stats — or `None` if unknown
    /// or unpriceable.
    pub async fn position(&self, hash: StrategyHash) -> Result<Option<Position>, SolventError> {
        let snapshot = self.registry.load();
        let Some(strategy) = snapshot.strategy_by_hash(hash).cloned() else {
            return Ok(None);
        };
        self.build(&strategy, true).await
    }

    /// A maker's active positions (list projection — no detail stats).
    pub async fn positions(&self, maker: MakerId) -> Result<Vec<Position>, SolventError> {
        let strategies: Vec<MakerStrategy> = self
            .registry
            .load()
            .strategies_for_maker(maker)
            .cloned()
            .collect();
        let mut positions = Vec::new();
        for strategy in &strategies {
            if let Some(position) = self.build(strategy, false).await? {
                positions.push(position);
            }
        }
        Ok(positions)
    }

    /// The active-maker roster: each maker's active-position count and shared liquidity.
    pub async fn roster(&self) -> Result<Vec<MakerSummary>, SolventError> {
        // Group active strategies by maker → (count, per-token committed balances).
        let grouped: HashMap<MakerId, (u64, Vec<(Address, U256)>)> = self
            .registry
            .load()
            .active_strategies()
            .filter(|s| matches!(s.curve, CurveSpec::Priceable { .. }))
            .map(|s| (s.key.maker, s))
            .into_grouping_map()
            .fold((0u64, Vec::new()), |(count, mut bals), _maker, s| {
                bals.extend(s.balances.iter().map(|(a, v)| (*a, *v)));
                (count + 1, bals)
            });

        let mut roster = Vec::with_capacity(grouped.len());
        for (maker, (holdings_count, holdings)) in grouped {
            let priced: Vec<(crate::primitives::asset::Token, U256)> = holdings
                .into_iter()
                .map(|(addr, amt)| (self.assets.token_or_default(addr), amt))
                .collect();
            roster.push(MakerSummary {
                maker: maker.0,
                active_positions: holdings_count,
                shared_liquidity_usd: self.valuation.tvl(&priced).await.map(Usd::to_f64),
            });
        }
        roster.sort_by_key(|entry| entry.maker);
        Ok(roster)
    }

    async fn build(
        &self,
        strategy: &MakerStrategy,
        detail: bool,
    ) -> Result<Option<Position>, SolventError> {
        let CurveSpec::Priceable { curve, fees_in_bps } = &strategy.curve else {
            return Ok(None);
        };
        let Some(pair) = strategy.pair() else {
            return Ok(None);
        };
        let (base_addr, quote_addr) = self.assets.base_quote(&pair);
        let (Some(base), Some(quote)) = (
            self.assets.token(&base_addr),
            self.assets.token(&quote_addr),
        ) else {
            return Ok(None);
        };
        let fee_bps = fee_in_bps(fees_in_bps);
        let range = self.range(curve, &pair, base_addr == pair.lo, &quote);

        let virtual_balances = self.valued(&strategy.balances).await;
        let actual = self.pullable(strategy).await?;
        let backed = is_backed(&strategy.balances, &actual);

        let metrics = self
            .metrics
            .position(strategy.key.strategy_hash, pair, self.since())
            .await?;
        let volume_usd = self.usd_volume(&metrics).await;
        let economics = economics(volume_usd, fee_bps, virtual_balances.total_usd);

        Ok(Some(Position {
            strategy_hash: format!("{:#x}", strategy.key.strategy_hash.0),
            pair: self.assets.pair_label(&pair),
            base,
            quote,
            maker: strategy.key.maker.0,
            curve: curve_label(curve).to_string(),
            pair_type: classify_pair(
                self.assets.is_stable(&pair.lo),
                self.assets.is_stable(&pair.hi),
                Some(curve_kind(curve)),
            ),
            state: if strategy.active { "active" } else { "docked" }.to_string(),
            fee_bps,
            since_block: None,
            docked_block: None,
            mid_price: None,
            range,
            balances: PositionBalances {
                split: split(&virtual_balances),
                virtual_balances,
                actual,
                backed,
                shortfall: None,
                coverage: None,
                opening: TokenAmounts {
                    entries: Vec::new(),
                    total_usd: None,
                },
            },
            economics,
            active_stats: detail.then_some(ActiveStats {
                fills_7d: metrics.fills,
                volume_7d_usd: volume_usd,
                quote_uptime_pct: metrics.quote_uptime_pct,
                last_fill_at: metrics.last_fill_at,
            }),
        }))
    }

    /// A `TokenAmounts` from committed balances, valued (all-or-nothing total).
    async fn valued(&self, balances: &BTreeMap<Address, U256>) -> TokenAmounts {
        let mut entries = Vec::with_capacity(balances.len());
        let mut priced = Vec::with_capacity(balances.len());
        for (addr, amount) in balances {
            let token = self.assets.token_or_default(*addr);
            entries.push(TokenAmount {
                amount: self
                    .valuation
                    .amount(*amount, token.address, token.decimals)
                    .await,
                token: token.clone(),
            });
            priced.push((token, *amount));
        }
        TokenAmounts {
            entries,
            total_usd: self.valuation.tvl(&priced).await.map(Usd::to_f64),
        }
    }

    /// The pullable (`min(committed, on-chain pullable)`) balances for a strategy, valued.
    async fn pullable(&self, strategy: &MakerStrategy) -> Result<TokenAmounts, SolventError> {
        let tokens: Vec<Address> = strategy.balances.keys().copied().collect();
        let holdings = self
            .balances
            .holdings(strategy.key.maker.0, &tokens)
            .await?;
        let capped: BTreeMap<Address, U256> = strategy
            .balances
            .iter()
            .map(|(addr, committed)| {
                let pullable = holdings.get(addr).map_or(U256::ZERO, |h| h.pullable);
                (*addr, (*committed).min(pullable))
            })
            .collect();
        Ok(self.valued(&capped).await)
    }

    async fn usd_volume(&self, metrics: &PositionMetrics) -> Option<f64> {
        let priced: Vec<_> = metrics
            .volume
            .iter()
            .map(|v| (self.assets.token_or_default(v.token), v.base_units))
            .collect();
        self.valuation.tvl(&priced).await.map(Usd::to_f64)
    }

    fn range(
        &self,
        curve: &Curve,
        pair: &TokenPair,
        base_is_lo: bool,
        quote: &crate::primitives::asset::Token,
    ) -> PriceRange {
        let dec_lo = self.assets.decimals(&pair.lo);
        let dec_hi = self.assets.decimals(&pair.hi);
        wire_range(
            price_range(curve, dec_lo, dec_hi, base_is_lo),
            &quote.symbol,
        )
    }

    fn since(&self) -> u64 {
        self.clock.now_unix().saturating_sub(WINDOW_DAYS * DAY)
    }
}

/// USD economics from window volume, the strategy's fee, and committed liquidity. APY annualizes the
/// window's fees over the committed liquidity: `fees/liquidity · 365/window`.
fn economics(volume_usd: Option<f64>, fee_bps: u32, liquidity_usd: Option<f64>) -> Economics {
    let fees_usd = volume_usd.map(|v| v * f64::from(fee_bps) / 10_000.0);
    let apy_pct = match (fees_usd, liquidity_usd) {
        (Some(fees), Some(liq)) if liq > 0.0 => {
            Some(fees / liq * (365.0 / WINDOW_DAYS as f64) * 100.0)
        }
        _ => None,
    };
    Economics {
        fees_usd,
        apy_pct,
        volume_usd,
    }
}

/// Whether every committed token is fully pullable on-chain (actual == committed).
fn is_backed(committed: &BTreeMap<Address, U256>, actual: &TokenAmounts) -> bool {
    committed.iter().all(|(addr, want)| {
        actual
            .entries
            .iter()
            .find(|e| e.token.address == *addr)
            .and_then(|e| e.amount.raw.parse::<U256>().ok())
            .is_some_and(|have| have >= *want)
    })
}

/// Per-token share of committed liquidity by USD value; empty if any token is unpriced.
fn split(balances: &TokenAmounts) -> Vec<Split> {
    let total: f64 = balances.total_usd.unwrap_or(0.0);
    if total <= 0.0 {
        return Vec::new();
    }
    balances
        .entries
        .iter()
        .filter_map(|entry| {
            entry.amount.usd.map(|usd| Split {
                token: entry.token.clone(),
                pct: usd / total * 100.0,
            })
        })
        .collect()
}

/// Map a decoded `PositionRange` to the wire shape, composing the human label with the quote symbol.
fn wire_range(range: PositionRange, quote_symbol: &str) -> PriceRange {
    let (kind, label) = match range.kind {
        RangeKind::Full => ("full", "Full range".to_string()),
        RangeKind::Bounded => (
            "bounded",
            match (&range.lower_price, &range.upper_price) {
                (Some(lo), Some(hi)) => format!("{lo}–{hi} {quote_symbol}"),
                _ => "Bounded".to_string(),
            },
        ),
        RangeKind::Peg => (
            "peg",
            match (&range.peg_price, range.below_pct) {
                (Some(peg), Some(pct)) => format!("±{pct:.2}% around {peg} {quote_symbol}"),
                (Some(peg), None) => format!("{peg} {quote_symbol}"),
                _ => "Pegged".to_string(),
            },
        ),
    };
    PriceRange {
        kind: kind.to_string(),
        lower_price: range.lower_price,
        upper_price: range.upper_price,
        peg_price: range.peg_price,
        below_pct: range.below_pct,
        above_pct: range.above_pct,
        label,
    }
}

fn curve_kind(curve: &Curve) -> CurveKind {
    match curve {
        Curve::Xyc => CurveKind::Xyc,
        Curve::Concentrate { .. } => CurveKind::Concentrated,
        Curve::Pegged(_) => CurveKind::Pegged,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use alloy_primitives::B256;
    use async_trait::async_trait;
    use rust_decimal::Decimal;

    use super::*;
    use crate::deps::balances::BalancesOracleError;
    use crate::deps::maker_metrics::{MakerMetrics, MakerMetricsError, TokenVolume};
    use crate::deps::routing::{PriceOracle, PriceOracleError};
    use crate::primitives::amount::Holdings;
    use crate::primitives::asset::{TokenList, TokenMeta};
    use crate::primitives::registry::{Curve, Snapshot, StrategyKey};
    use crate::primitives::UsdPrice;

    const USDC: u8 = 2; // lower address → the stable quote
    const WETH: u8 = 3;

    fn addr(n: u8) -> Address {
        Address::from([n; 20])
    }

    struct FakePrices(HashMap<Address, u64>);
    #[async_trait]
    impl PriceOracle for FakePrices {
        async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError> {
            self.0
                .get(&token)
                .map(|d| UsdPrice(Decimal::from(*d)))
                .ok_or(PriceOracleError::NotFound(token))
        }
    }

    struct FakeOracle(HashMap<Address, U256>);
    #[async_trait]
    impl BalancesOracle for FakeOracle {
        async fn holdings(
            &self,
            _: Address,
            tokens: &[Address],
        ) -> Result<BTreeMap<Address, Holdings>, BalancesOracleError> {
            Ok(tokens
                .iter()
                .filter_map(|t| {
                    self.0.get(t).map(|pullable| {
                        (
                            *t,
                            Holdings {
                                balance: *pullable,
                                pullable: *pullable,
                            },
                        )
                    })
                })
                .collect())
        }
    }

    struct FakeMetrics;
    #[async_trait]
    impl MakerMetricsStore for FakeMetrics {
        async fn maker(
            &self,
            _: MakerId,
            _: u64,
            _: u64,
        ) -> Result<MakerMetrics, MakerMetricsError> {
            unreachable!("roster does not read metrics")
        }
        async fn position(
            &self,
            _: StrategyHash,
            _: TokenPair,
            _: u64,
        ) -> Result<PositionMetrics, MakerMetricsError> {
            Ok(PositionMetrics {
                fills: 5,
                volume: vec![TokenVolume {
                    token: addr(WETH),
                    base_units: U256::from(1_000_000_000_000_000_000u128), // 1 WETH
                }],
                last_fill_at: Some(1_700),
                quote_uptime_pct: Some(75.0),
            })
        }
    }

    struct FixedClock;
    impl Clock for FixedClock {
        fn now_unix(&self) -> u64 {
            1_000_000
        }
    }

    fn xyc_strategy(maker: u8) -> MakerStrategy {
        let key = StrategyKey {
            maker: MakerId(addr(maker)),
            app: Address::ZERO,
            strategy_hash: StrategyHash(B256::from([maker; 32])),
        };
        let mut st = MakerStrategy::new(key, &[]);
        st.curve = CurveSpec::Priceable {
            curve: Curve::Xyc,
            fees_in_bps: vec![500_000], // 5 real bps
        };
        st.balances
            .insert(addr(WETH), U256::from(1_000_000_000_000_000_000u128)); // 1 WETH
        st.balances.insert(addr(USDC), U256::from(2_000_000_000u64)); // 2000 USDC
        st
    }

    fn service(snapshot: Snapshot, pullable: HashMap<Address, U256>) -> MakerService {
        let registry = Arc::new(SharedSnapshot::new(snapshot));
        let list = TokenList {
            name: "t".into(),
            tokens: vec![
                TokenMeta {
                    chain_id: 1,
                    address: addr(USDC),
                    symbol: "USDC".into(),
                    name: "USDC".into(),
                    decimals: 6,
                    logo_uri: None,
                    tags: vec!["stables".into()],
                },
                TokenMeta {
                    chain_id: 1,
                    address: addr(WETH),
                    symbol: "WETH".into(),
                    name: "WETH".into(),
                    decimals: 18,
                    logo_uri: None,
                    tags: vec![],
                },
            ],
        };
        let assets = Arc::new(AssetManager::new(list, Arc::clone(&registry)));
        let valuation = Arc::new(Valuation::new(Arc::new(FakePrices(HashMap::from([
            (addr(WETH), 2000),
            (addr(USDC), 1),
        ])))));
        MakerService::new(
            registry,
            assets,
            valuation,
            Arc::new(FakeMetrics),
            Arc::new(FakeOracle(pullable)),
            Arc::new(FixedClock),
        )
    }

    #[tokio::test]
    async fn builds_a_position() {
        // USDC fully pullable; WETH only half → not fully backed.
        let pullable = HashMap::from([
            (addr(WETH), U256::from(500_000_000_000_000_000u128)),
            (addr(USDC), U256::from(2_000_000_000u64)),
        ]);
        let svc = service(Snapshot::from_strategies([xyc_strategy(9)]), pullable);

        let p = svc
            .position(StrategyHash(B256::from([9; 32])))
            .await
            .unwrap()
            .expect("position");

        assert_eq!(p.curve, "XYC");
        assert_eq!(p.pair_type, crate::primitives::pool::PoolType::Volatile);
        assert_eq!(p.state, "active");
        assert_eq!(p.range.kind, "full");
        // virtual: 1 WETH @ $2000 + 2000 USDC @ $1 = $4000.
        assert_eq!(p.balances.virtual_balances.total_usd, Some(4000.0));
        assert!(!p.balances.backed, "WETH is only half pullable");
        // economics: window volume 1 WETH = $2000; fee 5bps → $1.
        assert_eq!(p.economics.volume_usd, Some(2000.0));
        assert_eq!(p.economics.fees_usd, Some(1.0));
        let stats = p.active_stats.expect("detail carries active_stats");
        assert_eq!(stats.fills_7d, 5);
        assert_eq!(stats.quote_uptime_pct, Some(75.0));
    }

    #[tokio::test]
    async fn roster_counts_and_values_makers() {
        let snapshot = Snapshot::from_strategies([xyc_strategy(9), xyc_strategy(10)]);
        let svc = service(snapshot, HashMap::new());

        let roster = svc.roster().await.unwrap();
        assert_eq!(roster.len(), 2);
        assert_eq!(roster[0].active_positions, 1);
        assert_eq!(roster[0].shared_liquidity_usd, Some(4000.0));
    }
}
