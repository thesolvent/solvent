//! The pool read-surface: aggregates active strategies into pool views by composing the registry
//! snapshot (`pool_stats`) with the asset manager (labels, token metadata, stable classification).

use std::sync::Arc;

use alloy_primitives::U256;

use crate::asset::AssetManager;
use crate::deps::ledger::clock::Clock;
use crate::deps::maker_metrics::{MakerMetricsStore, TokenVolume};
use crate::primitives::asset::Token;
use crate::primitives::pool::{classify_pair, CurveMix, Pool, PoolDetail, PoolMaker, PoolType};
use crate::primitives::registry::{
    curve_label, fee_in_bps, CurveSpec, MakerStrategy, PoolStats, Snapshot, TokenPair,
};
use crate::registry::SharedSnapshot;
use crate::valuation::Valuation;

/// Trailing window the pool KPIs describe; the yield annualises from it.
const WINDOW_SECS: u64 = 24 * 60 * 60;
const YEAR_SECS: f64 = 365.0 * 24.0 * 60.0 * 60.0;
const BPS_PER_UNIT: f64 = 10_000.0;

pub struct PoolService {
    registry: Arc<SharedSnapshot>,
    assets: Arc<AssetManager>,
    valuation: Arc<Valuation>,
    metrics: Arc<dyn MakerMetricsStore>,
    clock: Arc<dyn Clock>,
}

impl PoolService {
    pub fn new(
        registry: Arc<SharedSnapshot>,
        assets: Arc<AssetManager>,
        valuation: Arc<Valuation>,
        metrics: Arc<dyn MakerMetricsStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            registry,
            assets,
            valuation,
            metrics,
            clock,
        }
    }

    /// One row per active pair, registry-derived. `volume`/`fills`/`apr` stay `None`/`0` until their
    /// data sources exist (M4). A pair whose tokens are missing from the catalog is skipped.
    pub async fn pools(&self) -> Vec<Pool> {
        let snapshot = self.registry.load();
        let mut pools = Vec::new();
        for (pair, stats) in snapshot.pool_stats() {
            if let Some(pool) = self.assemble(&snapshot, &pair, &stats).await {
                pools.push(pool);
            }
        }
        pools
    }

    async fn assemble(
        &self,
        snapshot: &Snapshot,
        pair: &TokenPair,
        stats: &PoolStats,
    ) -> Option<Pool> {
        let (base_addr, quote_addr) = self.assets.base_quote(pair);
        // A metrics outage leaves the pool listed with no activity rather than hiding it.
        let activity = self.metrics.pair_activity(*pair, self.since()).await.ok();
        let volume_24h_usd = match &activity {
            Some(a) => self.usd_of(&a.volume).await,
            None => None,
        };
        let tvl_usd = self.pair_tvl(snapshot, pair).await;
        Some(Pool {
            pair: self.assets.pair_label(pair),
            base: self.assets.token(&base_addr)?,
            quote: self.assets.token(&quote_addr)?,
            pool_type: self.classify(pair, stats),
            maker_count: stats.maker_count as u64,
            min_spread_bps: stats.min_fee_bps,
            max_spread_bps: stats.max_fee_bps,
            popular_fee_tier: fee_tier(stats.popular_fee_bps),
            curve_mix: CurveMix {
                xyc: stats.curve_mix.xyc as u64,
                concentrated: stats.curve_mix.concentrated as u64,
                pegged: stats.curve_mix.pegged as u64,
            },
            tvl_usd,
            volume_24h_usd,
            fills_24h: activity.as_ref().map_or(0, |a| a.fills),
            apr_pct: yield_pct(volume_24h_usd, tvl_usd, stats.popular_fee_bps),
        })
    }

    /// The USD value of a per-token flow (all-or-nothing — `None` if any token is unpriced).
    async fn usd_of(&self, volume: &[TokenVolume]) -> Option<f64> {
        let priced: Vec<_> = volume
            .iter()
            .map(|v| (self.assets.token_or_default(v.token), v.base_units))
            .collect();
        self.valuation.tvl_usd(&priced).await
    }

    fn since(&self) -> u64 {
        self.clock.now_unix().saturating_sub(WINDOW_SECS)
    }

    /// The pair's total value locked: every active strategy's committed balances, valued. `None` if
    /// any token is unpriced or missing from the catalog — a partial TVL would understate the pool.
    async fn pair_tvl(&self, snapshot: &Snapshot, pair: &TokenPair) -> Option<f64> {
        let mut holdings: Vec<(Token, U256)> = Vec::new();
        for strategy in snapshot.active_strategies_for_pair(*pair) {
            for (address, balance) in &strategy.balances {
                holdings.push((self.assets.token(address)?, *balance));
            }
        }
        self.valuation.tvl_usd(&holdings).await
    }

    /// Both tokens stable → `Stable`; else pegged-dominant → `Correlated`; else `Volatile`.
    fn classify(&self, pair: &TokenPair, stats: &PoolStats) -> PoolType {
        classify_pair(
            self.assets.is_stable(&pair.lo),
            self.assets.is_stable(&pair.hi),
            stats.curve_mix.dominant(),
        )
    }

    /// Full detail for `pair`, or `None` if it has no active pool. The row is the same as the list's;
    /// the roster is registry-derived (actual/pullable on-chain and recent fills join later).
    pub async fn pool_detail(&self, pair: &TokenPair) -> Option<PoolDetail> {
        let snapshot = self.registry.load();
        let stats = snapshot.pool_stats();
        let pool = self.assemble(&snapshot, pair, stats.get(pair)?).await?;
        let mut makers = Vec::new();
        for strategy in snapshot.active_strategies_for_pair(*pair) {
            if let Some(maker) = self.pool_maker(strategy).await {
                makers.push(maker);
            }
        }
        Some(PoolDetail { pool, makers })
    }

    /// One roster entry: curve, fee, and committed balances (valued). `None` for an unpriceable
    /// strategy. `total_usd` is `None` if any of the maker's tokens is unpriced.
    async fn pool_maker(&self, strategy: &MakerStrategy) -> Option<PoolMaker> {
        let CurveSpec::Priceable { curve, fees_in_bps } = &strategy.curve else {
            return None;
        };
        let holdings: Vec<(Token, U256)> = strategy
            .balances
            .iter()
            .filter_map(|(address, balance)| Some((self.assets.token(address)?, *balance)))
            .collect();
        Some(PoolMaker {
            maker: strategy.key.maker.0,
            strategy_hash: format!("{:#x}", strategy.key.strategy_hash.0),
            curve: curve_label(curve).to_string(),
            fee_bps: fee_in_bps(fees_in_bps),
            virtual_balances: self.valuation.priced_amounts(&holdings).await,
        })
    }
}

/// Real bps → a percentage tier string, e.g. `5` → `"0.05%"`.
fn fee_tier(bps: u32) -> String {
    format!("{:.2}%", bps as f64 / 100.0)
}

/// Fees the window earned over the capital backing them, annualised.
///
/// Volume is measured on the delivered side, which stands in for the notional the maker fee was
/// charged on. `None` unless both sides are known and the pool holds value.
fn yield_pct(volume_usd: Option<f64>, tvl_usd: Option<f64>, fee_bps: u32) -> Option<f64> {
    let (volume, tvl) = (volume_usd?, tvl_usd?);
    if tvl <= 0.0 {
        return None;
    }
    let fees = volume * (f64::from(fee_bps) / BPS_PER_UNIT);
    Some(fees / tvl * (YEAR_SECS / WINDOW_SECS as f64) * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deps::maker_metrics::{
        MakerMetrics, MakerMetricsError, PairMetrics, PositionMetrics,
    };
    use crate::deps::routing::{PriceOracle, PriceOracleError};
    use crate::primitives::asset::{TokenList, TokenMeta};
    use crate::primitives::registry::{
        Curve, CurveSpec, MakerStrategy, PeggedParams, Snapshot, StrategyKey,
    };
    use crate::primitives::{MakerId, StrategyHash, UsdPrice};
    use alloy_primitives::{Address, B256, U256};
    use async_trait::async_trait;
    use rust_decimal::Decimal;
    use std::collections::BTreeMap;

    /// Pools list without regard to trading history, so the metrics port stays silent here; the
    /// yield itself is covered by `yield_pct`'s own tests.
    struct NoActivity;
    #[async_trait]
    impl MakerMetricsStore for NoActivity {
        async fn maker(
            &self,
            _: MakerId,
            _: u64,
            _: u64,
        ) -> Result<MakerMetrics, MakerMetricsError> {
            Err(MakerMetricsError::Db("unused".into()))
        }
        async fn position(
            &self,
            _: StrategyHash,
            _: TokenPair,
            _: u64,
        ) -> Result<PositionMetrics, MakerMetricsError> {
            Err(MakerMetricsError::Db("unused".into()))
        }
        async fn pair_fills(&self, _: &[TokenPair], _: u64) -> Result<u64, MakerMetricsError> {
            Ok(0)
        }
        async fn pair_activity(
            &self,
            _: TokenPair,
            _: u64,
        ) -> Result<PairMetrics, MakerMetricsError> {
            Ok(PairMetrics {
                fills: 0,
                volume: vec![],
            })
        }
    }

    struct FixedClock;
    impl Clock for FixedClock {
        fn now_unix(&self) -> u64 {
            1_000_000
        }
    }

    fn metrics() -> Arc<dyn MakerMetricsStore> {
        Arc::new(NoActivity)
    }

    fn clock() -> Arc<dyn Clock> {
        Arc::new(FixedClock)
    }

    fn addr(n: u8) -> Address {
        Address::from([n; 20])
    }

    struct FakePrices(BTreeMap<Address, UsdPrice>);

    #[async_trait]
    impl PriceOracle for FakePrices {
        async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError> {
            self.0
                .get(&token)
                .copied()
                .ok_or(PriceOracleError::NotFound(token))
        }
    }

    fn valuation(prices: &[(Address, u64)]) -> Arc<Valuation> {
        let map = prices
            .iter()
            .map(|(a, dollars)| (*a, UsdPrice(Decimal::from(*dollars))))
            .collect();
        Arc::new(Valuation::new(Arc::new(FakePrices(map))))
    }

    fn meta(n: u8, symbol: &str, decimals: u8, stable: bool) -> TokenMeta {
        TokenMeta {
            chain_id: 31337,
            address: addr(n),
            symbol: symbol.to_string(),
            name: symbol.to_string(),
            decimals,
            logo_uri: None,
            tags: if stable {
                vec!["stables".to_string()]
            } else {
                vec![]
            },
        }
    }

    fn peg() -> PeggedParams {
        let one = U256::from(1u64);
        PeggedParams {
            x0: one,
            y0: one,
            linear_width: one,
            rate_lt: one,
            rate_gt: one,
        }
    }

    fn strat(s: u8, a: Address, b: Address, curve: Curve) -> MakerStrategy {
        let key = StrategyKey {
            maker: MakerId(Address::from([s; 20])),
            app: Address::ZERO,
            strategy_hash: StrategyHash(B256::from([s; 32])),
        };
        let mut st = MakerStrategy::new(key, &[]);
        st.curve = CurveSpec::Priceable {
            curve,
            fees_in_bps: vec![500_000],
        };
        st.balances.insert(a, U256::from(1u64));
        st.balances.insert(b, U256::from(1u64));
        st
    }

    #[tokio::test]
    async fn classifies_pools_and_orders_base_quote() {
        let list = TokenList {
            name: "test".to_string(),
            tokens: vec![
                meta(1, "USDC", 6, true),
                meta(2, "USDT", 6, true),
                meta(3, "WETH", 18, false),
            ],
        };
        let snap = Snapshot::from_strategies([
            strat(0, addr(1), addr(2), Curve::Pegged(peg())), // USDC/USDT — both stable
            strat(1, addr(3), addr(1), Curve::Xyc),           // WETH/USDC
        ]);
        let registry = Arc::new(SharedSnapshot::new(snap));
        let assets = Arc::new(AssetManager::new(list, Arc::clone(&registry)));
        let pools = PoolService::new(registry, assets, valuation(&[]), metrics(), clock())
            .pools()
            .await;

        let stable = pools
            .iter()
            .find(|p| p.pool_type == PoolType::Stable)
            .unwrap();
        assert_eq!(stable.pair, "USDC/USDT");

        let volatile = pools
            .iter()
            .find(|p| p.pool_type == PoolType::Volatile)
            .unwrap();
        assert_eq!(volatile.pair, "WETH/USDC"); // stable is the quote
        assert_eq!(volatile.base.symbol, "WETH");
        assert_eq!(volatile.quote.symbol, "USDC");
        assert_eq!(volatile.popular_fee_tier, "0.05%");
    }

    #[tokio::test]
    async fn pool_detail_has_roster_with_decimal_balances() {
        let list = TokenList {
            name: "test".to_string(),
            tokens: vec![meta(3, "WETH", 18, false), meta(1, "USDC", 6, true)],
        };
        let mut strategy = strat(1, addr(3), addr(1), Curve::Xyc);
        strategy
            .balances
            .insert(addr(3), U256::from(4_000_000_000_000_000_000u128)); // 4 WETH (18 dec)
        strategy
            .balances
            .insert(addr(1), U256::from(12_000_000_000u64)); // 12000 USDC (6 dec)
        let registry = Arc::new(SharedSnapshot::new(Snapshot::from_strategies([strategy])));
        let assets = Arc::new(AssetManager::new(list, Arc::clone(&registry)));
        let svc = PoolService::new(registry, assets, valuation(&[]), metrics(), clock());

        let detail = svc
            .pool_detail(&TokenPair::new(addr(3), addr(1)))
            .await
            .unwrap();
        assert_eq!(detail.pool.pair, "WETH/USDC");
        assert_eq!(detail.pool.maker_count, 1);
        let maker = &detail.makers[0];
        assert_eq!(maker.curve, "XYC");
        let amount = |sym: &str| {
            maker
                .virtual_balances
                .entries
                .iter()
                .find(|e| e.token.symbol == sym)
                .unwrap()
                .amount
                .display
                .clone()
        };
        assert_eq!(amount("WETH"), "4");
        assert_eq!(amount("USDC"), "12000");

        // an unknown pair has no detail
        assert!(svc
            .pool_detail(&TokenPair::new(addr(1), addr(2)))
            .await
            .is_none());
    }

    #[tokio::test]
    async fn values_pool_tvl_from_registry_balances() {
        let list = TokenList {
            name: "test".to_string(),
            tokens: vec![meta(3, "WETH", 18, false), meta(1, "USDC", 6, true)],
        };
        let mut strategy = strat(1, addr(3), addr(1), Curve::Xyc);
        strategy
            .balances
            .insert(addr(3), U256::from(4_000_000_000_000_000_000u128)); // 4 WETH
        strategy
            .balances
            .insert(addr(1), U256::from(12_000_000_000u64)); // 12000 USDC
        let registry = Arc::new(SharedSnapshot::new(Snapshot::from_strategies([strategy])));
        let assets = Arc::new(AssetManager::new(list, Arc::clone(&registry)));
        // WETH $2000, USDC $1 → 4·2000 + 12000·1 = 20000
        let svc = PoolService::new(
            registry,
            assets,
            valuation(&[(addr(3), 2000), (addr(1), 1)]),
            metrics(),
            clock(),
        );

        let pools = svc.pools().await;
        assert_eq!(pools.len(), 1);
        assert_eq!(pools[0].tvl_usd, Some(20000.0));

        let detail = svc
            .pool_detail(&TokenPair::new(addr(3), addr(1)))
            .await
            .unwrap();
        assert_eq!(detail.makers[0].virtual_balances.total_usd, Some(20000.0));
    }

    #[test]
    fn yield_annualises_the_window_fee_over_backing() {
        // $10k traded through a 30bps pool earns $30 a day on $100k of depth: 0.03% daily, x365.
        let apr = yield_pct(Some(10_000.0), Some(100_000.0), 30).unwrap();
        assert!((apr - 10.95).abs() < 1e-9, "{apr}");
    }

    #[test]
    fn yield_is_unknown_without_both_sides_or_backing() {
        assert_eq!(yield_pct(None, Some(100_000.0), 30), None);
        assert_eq!(yield_pct(Some(10_000.0), None, 30), None);
        // No backing would divide by zero and report an infinite return.
        assert_eq!(yield_pct(Some(10_000.0), Some(0.0), 30), None);
    }
}
