//! The pool read-surface: aggregates active strategies into pool views by composing the registry
//! snapshot (`pool_stats`) with the asset manager (labels, token metadata, stable classification).

use std::sync::Arc;

use alloy_primitives::{Address, U256};
use futures::future::join_all;

use crate::asset::AssetManager;
use crate::deps::balances::BalancesOracle;
use crate::deps::ledger::clock::Clock;
use crate::deps::maker_metrics::{MakerMetricsStore, TokenVolume};
use crate::primitives::amount::TokenAmounts;
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
    balances: Arc<dyn BalancesOracle>,
    clock: Arc<dyn Clock>,
}

impl PoolService {
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
        let value = self.pair_value(snapshot, pair).await;
        let tvl_usd = value.usd;
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
            tvl_change_24h_pct: value.change_24h_pct,
            volume_24h_usd,
            fills_24h: activity.as_ref().map_or(0, |a| a.fills),
            apr_pct: yield_pct(volume_24h_usd, tvl_usd, stats.popular_fee_bps),
        })
    }

    /// What the maker can actually deliver against a commitment: the pullable amount, which Aqua
    /// caps at the allowance, and never more than was committed.
    async fn deliverable(
        &self,
        maker: Address,
        committed: &[(Token, U256)],
    ) -> Option<TokenAmounts> {
        let tokens: Vec<Address> = committed.iter().map(|(token, _)| token.address).collect();
        let holdings = self.balances.holdings(maker, &tokens).await.ok()?;
        let capped: Vec<(Token, U256)> = committed
            .iter()
            .map(|(token, amount)| {
                let pullable = holdings
                    .get(&token.address)
                    .map_or(U256::ZERO, |h| h.pullable);
                (token.clone(), (*amount).min(pullable))
            })
            .collect();
        Some(self.valuation.priced_amounts(&capped).await)
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

    /// The pair's committed balances, valued, with how that value moved over the last day. Either
    /// figure is `None` if a token is unpriced or missing from the catalog — a partial total would
    /// understate the pool.
    async fn pair_value(&self, snapshot: &Snapshot, pair: &TokenPair) -> PairValue {
        let mut holdings: Vec<(Token, U256)> = Vec::new();
        for strategy in snapshot.active_strategies_for_pair(*pair) {
            for (address, balance) in &strategy.balances {
                let Some(token) = self.assets.token(address) else {
                    return PairValue::UNKNOWN;
                };
                holdings.push((token, *balance));
            }
        }

        let priced = self.valuation.priced_amounts(&holdings).await;
        PairValue {
            usd: priced.total_usd,
            change_24h_pct: self.weighted_change(&priced).await,
        }
    }

    /// Each token's 24h move, weighted by what the pool holds of it. `None` unless every token
    /// reports one — averaging over the ones that do would quietly misstate the rest.
    async fn weighted_change(&self, priced: &TokenAmounts) -> Option<f64> {
        let total = priced.total_usd?;
        if total <= 0.0 {
            return None;
        }
        let mut weighted = 0.0;
        for entry in &priced.entries {
            let value = entry.amount.usd?;
            weighted += value * self.valuation.change_24h(entry.token.address).await?;
        }
        Some(weighted / total)
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
        // Each row costs its own chain read for the deliverable balance, so the roster is built
        // concurrently rather than one round-trip after another.
        let makers = join_all(
            snapshot
                .active_strategies_for_pair(*pair)
                .map(|strategy| self.pool_maker(strategy)),
        )
        .await
        .into_iter()
        .flatten()
        .collect();
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
            actual: self.deliverable(strategy.key.maker.0, &holdings).await,
        })
    }
}

/// Real bps → a percentage tier string, e.g. `5` → `"0.05%"`.
fn fee_tier(bps: u32) -> String {
    format!("{:.2}%", bps as f64 / 100.0)
}

/// A pair's committed value and how it moved over the last day.
struct PairValue {
    usd: Option<f64>,
    change_24h_pct: Option<f64>,
}

impl PairValue {
    const UNKNOWN: Self = Self {
        usd: None,
        change_24h_pct: None,
    };
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
    use crate::deps::balances::BalancesOracleError;
    use crate::deps::maker_metrics::{
        MakerMetrics, MakerMetricsError, PairMetrics, PositionMetrics,
    };
    use crate::deps::routing::{PriceOracle, PriceOracleError};
    use crate::primitives::amount::Holdings;
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
            _: std::ops::Range<u64>,
        ) -> Result<MakerMetrics, MakerMetricsError> {
            Err(MakerMetricsError::Db("unused".into()))
        }
        async fn position(
            &self,
            _: StrategyHash,
            _: TokenPair,
            _: std::ops::Range<u64>,
        ) -> Result<PositionMetrics, MakerMetricsError> {
            Err(MakerMetricsError::Db("unused".into()))
        }
        async fn pair_fills(
            &self,
            _: &[TokenPair],
            _: std::ops::Range<u64>,
        ) -> Result<u64, MakerMetricsError> {
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

    /// Reports the same pullable amount for every token the caller asks about.
    struct Pullable(u128);
    #[async_trait]
    impl BalancesOracle for Pullable {
        async fn holdings(
            &self,
            _: Address,
            tokens: &[Address],
        ) -> Result<BTreeMap<Address, Holdings>, BalancesOracleError> {
            Ok(tokens
                .iter()
                .map(|token| {
                    let amount = U256::from(self.0);
                    (
                        *token,
                        Holdings {
                            balance: amount,
                            pullable: amount,
                        },
                    )
                })
                .collect())
        }
    }

    fn pullable(amount: u128) -> Arc<dyn BalancesOracle> {
        Arc::new(Pullable(amount))
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
        let pools = PoolService::new(
            registry,
            assets,
            valuation(&[]),
            metrics(),
            pullable(0),
            clock(),
        )
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
        let svc = PoolService::new(
            registry,
            assets,
            valuation(&[]),
            metrics(),
            pullable(0),
            clock(),
        );

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
            pullable(0),
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

    #[tokio::test]
    async fn actual_is_the_committed_amount_capped_by_what_aqua_may_pull() {
        let list = TokenList {
            name: "test".to_string(),
            tokens: vec![meta(1, "USDC", 6, true), meta(3, "WETH", 18, false)],
        };
        let mut strategy = strat(0, addr(3), addr(1), Curve::Xyc);
        strategy
            .balances
            .insert(addr(3), U256::from(10_000_000_000_000_000_000u128)); // 10 WETH committed
        let registry = Arc::new(SharedSnapshot::new(Snapshot::from_strategies([strategy])));
        let assets = Arc::new(AssetManager::new(list, Arc::clone(&registry)));

        // The maker may only be pulled for 4 WETH, so that is all the pool can count on.
        let svc = PoolService::new(
            registry,
            assets,
            valuation(&[(addr(3), 2000), (addr(1), 1)]),
            metrics(),
            pullable(4_000_000_000_000_000_000),
            clock(),
        );

        let detail = svc
            .pool_detail(&TokenPair::new(addr(3), addr(1)))
            .await
            .unwrap();
        let maker = &detail.makers[0];
        let committed = maker.virtual_balances.total_usd.unwrap();
        let deliverable = maker.actual.as_ref().unwrap().total_usd.unwrap();

        assert!((committed - 20_000.0).abs() < 0.01, "{committed}");
        // 4 of the 10 committed WETH are pullable, so only that share is deliverable.
        assert!((deliverable - 8_000.0).abs() < 0.01, "{deliverable}");
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
