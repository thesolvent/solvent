//! The pool depth read-surface: the combined executable liquidity of a pair, plotted like an order
//! book the pool doesn't have. It reconstructs the book by handing the router's own machinery
//! ([`select`] + [`solve`]) a sweep of trade sizes: each size's optimal split across every active
//! maker is one point on the curve. No new curve math — depth is the split, plotted, not solved once.

use std::sync::Arc;

use alloy_primitives::{Address, B256, U256};

use crate::asset::AssetManager;
use crate::ledger::BudgetCache;
use crate::primitives::amount::format_units;
use crate::primitives::pool::{DepthPoint, PoolDepth, Side};
use crate::primitives::pricing::Ratio;
use crate::primitives::registry::TokenPair;
use crate::primitives::routing::RouteRequest;
use crate::primitives::IntentId;
use crate::registry::SharedSnapshot;
use crate::routing::{select, solve, Candidate, Split};

/// Places kept when rendering a human price string.
const PRICE_DECIMALS: u8 = 6;
/// The impact buckets the curve is anchored to (bps): 0.1% … 10%.
const IMPACT_LADDER_BPS: [u64; 6] = [10, 50, 100, 200, 500, 1000];
/// Bisection steps to land a trade size on an impact bucket.
const BISECT_ITERS: usize = 48;

/// Serves `GET /pools/depth`. Composes the registry (which makers quote the pair), the budget cache
/// (each maker's synced executable cap, read lock-free with no RPC), and the asset manager
/// (orientation + token decimals). Fully in-memory — the chain reads live in the cache's sync tick.
pub struct DepthService {
    registry: Arc<SharedSnapshot>,
    budgets: Arc<BudgetCache>,
    assets: Arc<AssetManager>,
}

impl DepthService {
    pub fn new(
        registry: Arc<SharedSnapshot>,
        budgets: Arc<BudgetCache>,
        assets: Arc<AssetManager>,
    ) -> Self {
        Self {
            registry,
            budgets,
            assets,
        }
    }

    /// The executable-liquidity depth curve for `pair` on `side`, or `None` when the pair has no
    /// active pool. `range` is accepted as a client-side zoom hint; the curve is always computed from
    /// the current curves + synced balances (no history, no per-request RPC).
    pub fn depth(&self, pair: &TokenPair, side: Side, _range: Option<&str>) -> Option<PoolDepth> {
        let snapshot = self.registry.load();
        // No active pool for this pair → no depth (404).
        snapshot.active_strategies_for_pair(*pair).next()?;
        // `sell` sources the base out to the quote; `buy` the reverse. The curves are asymmetric.
        let (base, quote) = self.assets.base_quote(pair);
        let (token_in, token_out) = match side {
            Side::Sell => (base, quote),
            Side::Buy => (quote, base),
        };
        // The whole synced caps snapshot; `select` reads only the accounts it needs. `k = usize::MAX`
        // keeps the whole book (candidates are amount-independent — no funnel here).
        let caps = self.budgets.load();
        let candidates = select(
            &snapshot,
            &caps,
            &request(token_in, token_out, U256::from(1u64)),
            usize::MAX,
        )
        .chosen;
        let axis_title = format!("Cumulative {} out vs price impact", self.symbol(token_out));
        Some(build_curve(
            &candidates,
            token_in,
            token_out,
            self.decimals(token_in),
            self.decimals(token_out),
            axis_title,
        ))
    }

    fn decimals(&self, token: Address) -> u8 {
        self.assets.token(&token).map(|t| t.decimals).unwrap_or(18)
    }

    fn symbol(&self, token: Address) -> String {
        self.assets
            .token(&token)
            .map(|t| t.symbol)
            .unwrap_or_else(|| "token".to_string())
    }
}

/// The depth sweep is parameterized by **output**: `amount` is a target `token_out` amount. That
/// makes the book's capacity `Σ cap_out` (an output ceiling) the natural x-axis scale, and keeps the
/// solve well-conditioned — an exact-in size would have no bound when a maker's cap dwarfs its curve.
fn request(token_in: Address, token_out: Address, amount: U256) -> RouteRequest {
    RouteRequest {
        intent: IntentId(B256::ZERO),
        token_in,
        token_out,
        amount,
        exact_in: false,
    }
}

/// Sweep the impact ladder: for each bucket, the output level whose blended price impact reaches it,
/// reported as its `(input, output)`. The tip (`best_price`) is the price at a tiny output probe;
/// the ladder stops at the first bucket the book is too shallow to reach.
fn build_curve(
    candidates: &[Candidate],
    token_in: Address,
    token_out: Address,
    in_dec: u8,
    out_dec: u8,
    axis_title: String,
) -> PoolDepth {
    let empty = PoolDepth {
        axis_title: axis_title.clone(),
        best_price: "0".to_string(),
        points: Vec::new(),
    };
    // The book's output capacity: the sum of every maker's deliverable cap. It sets the sweep scale.
    let capacity = candidates
        .iter()
        .fold(U256::ZERO, |sum, c| sum.saturating_add(c.cap_out));
    if capacity.is_zero() {
        return empty;
    }
    // Stay just under capacity so the top-of-range solve is always feasible.
    let ceiling = capacity
        .saturating_sub(capacity / U256::from(1000u64))
        .max(U256::from(1u64));
    let probe = (capacity / U256::from(1_000_000u64)).max(U256::from(1u64));
    let (Some(tip), Some(best)) = tip_price(candidates, token_in, token_out, probe) else {
        return empty;
    };
    let best_price = price_string(tip.amount_out, out_dec, tip.amount_in, in_dec);

    let mut points = Vec::new();
    for target_bps in IMPACT_LADDER_BPS {
        match size_for_impact(candidates, token_in, token_out, &best, target_bps, ceiling) {
            Some(split) => points.push(point(&split, &best, in_dec, out_dec)),
            None => break, // the book is too shallow to reach this impact
        }
    }
    PoolDepth {
        axis_title,
        best_price,
        points,
    }
}

/// The split for a tiny output `probe` and its price ratio — the curve's tip (near-zero impact).
fn tip_price(
    candidates: &[Candidate],
    token_in: Address,
    token_out: Address,
    probe: U256,
) -> (Option<Split>, Option<Ratio>) {
    match solve(candidates, &request(token_in, token_out, probe), None) {
        Some(split) => {
            let ratio = Ratio::new(split.amount_out, split.amount_in);
            (Some(split), ratio)
        }
        None => (None, None),
    }
}

/// Bisect the output level in `[1, ceiling]` for the smallest one whose impact reaches `target_bps`,
/// or `None` when even the whole book can't. Impact is monotone in the output for a concave book.
fn size_for_impact(
    candidates: &[Candidate],
    token_in: Address,
    token_out: Address,
    best: &Ratio,
    target_bps: u64,
    ceiling: U256,
) -> Option<Split> {
    let at = |output: U256| solve(candidates, &request(token_in, token_out, output), None);
    let top = at(ceiling)?;
    if impact_bps(&top, best) < target_bps {
        return None;
    }
    let (mut lo, mut hi, mut hit) = (U256::from(1u64), ceiling, top);
    for _ in 0..BISECT_ITERS {
        if hi.saturating_sub(lo) <= U256::from(1u64) {
            break;
        }
        let mid = lo.saturating_add(hi.saturating_sub(lo) / U256::from(2u64));
        match at(mid) {
            Some(split) if impact_bps(&split, best) >= target_bps => {
                hi = mid;
                hit = split;
            }
            Some(_) => lo = mid,
            None => hi = mid,
        }
    }
    Some(hit)
}

/// How far a split's blended price sits below the tip, in bps (`|effective − best| / best`).
fn impact_bps(split: &Split, best: &Ratio) -> u64 {
    match Ratio::new(split.amount_out, split.amount_in) {
        Some(effective) => effective.rel_diff_bps(best),
        None => 0,
    }
}

fn point(split: &Split, best: &Ratio, in_dec: u8, out_dec: u8) -> DepthPoint {
    DepthPoint {
        trade_size: split.amount_in.to_string(),
        output: split.amount_out.to_string(),
        effective_price: price_string(split.amount_out, out_dec, split.amount_in, in_dec),
        impact_pct: impact_bps(split, best) as f64 / 100.0,
        makers_used: split.legs.len() as u64,
    }
}

/// The human price `out/in` as a decimal string: `(out·10^in_dec)/(in·10^out_dec)`, floored to
/// `PRICE_DECIMALS` places. The two tokens' decimals differ, so both sides scale to a common base.
fn price_string(out: U256, out_dec: u8, input: U256, in_dec: u8) -> String {
    let num = out
        .saturating_mul(pow10(in_dec))
        .saturating_mul(pow10(PRICE_DECIMALS));
    let den = input.saturating_mul(pow10(out_dec));
    match Ratio::new(num, den).and_then(|r| r.floor()) {
        Some(scaled) => format_units(scaled, PRICE_DECIMALS),
        None => "0".to_string(),
    }
}

fn pow10(n: u8) -> U256 {
    U256::from(10u64).pow(U256::from(n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deps::ledger::{BudgetSource, BudgetSourceError};
    use crate::ledger::BudgetCache;
    use crate::primitives::asset::{TokenList, TokenMeta};
    use crate::primitives::ledger::AccountKey;
    use crate::primitives::registry::{Curve, CurveSpec, MakerStrategy, Snapshot, StrategyKey};
    use crate::primitives::{MakerId, StrategyHash};

    fn addr(n: u8) -> Address {
        Address::from([n; 20])
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

    /// An XYC strategy over WETH(2)/USDC(1) with the given raw reserves.
    fn xyc(hash: u8, weth: U256, usdc: U256) -> MakerStrategy {
        let key = StrategyKey {
            maker: MakerId(addr(hash)),
            app: Address::ZERO,
            strategy_hash: StrategyHash(B256::from([hash; 32])),
        };
        let mut strategy = MakerStrategy::new(key, &[]);
        strategy.curve = CurveSpec::Priceable {
            curve: Curve::Xyc,
            fees_in_bps: vec![],
        };
        strategy.balances.insert(addr(2), weth); // token_in
        strategy.balances.insert(addr(1), usdc); // token_out (payout)
        strategy
    }

    /// A docked (inactive) XYC strategy — keeps its balances but shouldn't be routable.
    fn docked(hash: u8, weth: U256, usdc: U256) -> MakerStrategy {
        let mut strategy = xyc(hash, weth, usdc);
        strategy.active = false;
        strategy
    }

    /// An active strategy on the pair whose curve can't be priced.
    fn unpriceable(hash: u8) -> MakerStrategy {
        let mut strategy = xyc(hash, e(100, 18), e(300_000, 6));
        strategy.curve = CurveSpec::Unsupported;
        strategy
    }

    /// Mirrors the real source: a strategy virtual is its registry balance, a wallet is a configured
    /// pullable `cap`. So the deliverable cap = `min(cap, registry balance)` — a huge `cap` lets the
    /// curve (reserve) bind; a small one caps the book below it.
    struct FakeBudget {
        registry: Arc<SharedSnapshot>,
        cap: U256,
    }

    #[async_trait::async_trait]
    impl BudgetSource for FakeBudget {
        async fn budget(&self, account: &AccountKey) -> Result<U256, BudgetSourceError> {
            match account {
                AccountKey::WalletBudget { .. } => Ok(self.cap),
                AccountKey::StrategyVirtual {
                    maker,
                    strategy_hash,
                    token,
                } => {
                    let key = StrategyKey {
                        maker: *maker,
                        app: Address::ZERO,
                        strategy_hash: *strategy_hash,
                    };
                    Ok(self
                        .registry
                        .load()
                        .strategy(&key)
                        .filter(|s| s.active)
                        .map(|s| s.balance(token))
                        .unwrap_or(U256::ZERO))
                }
            }
        }
    }

    async fn service(strategies: Vec<MakerStrategy>, cap: U256) -> DepthService {
        let list = TokenList {
            name: "test".to_string(),
            tokens: vec![meta(1, "USDC", 6, true), meta(2, "WETH", 18, false)],
        };
        let registry = Arc::new(SharedSnapshot::new(Snapshot::from_strategies(strategies)));
        let assets = Arc::new(AssetManager::new(list, Arc::clone(&registry)));
        let source: Arc<dyn BudgetSource> = Arc::new(FakeBudget {
            registry: Arc::clone(&registry),
            cap,
        });
        let budgets = Arc::new(BudgetCache::new(source, Arc::clone(&registry)));
        budgets.refresh().await.unwrap();
        DepthService::new(registry, budgets, assets)
    }

    fn e(n: u64, dp: u32) -> U256 {
        U256::from(n) * U256::from(10u64).pow(U256::from(dp))
    }

    #[tokio::test]
    async fn depth_curve_rises_and_deepens_with_size() {
        // Two XYC makers of different depth, priced ~3000 USDC/WETH, caps that never bind.
        let strategies = vec![
            xyc(3, e(100, 18), e(300_000, 6)),
            xyc(4, e(200, 18), e(600_000, 6)),
        ];
        let svc = service(strategies, e(1, 30)).await;
        let pair = TokenPair::new(addr(1), addr(2));

        let depth = svc.depth(&pair, Side::Sell, None).unwrap();
        assert!(!depth.points.is_empty(), "a priced pool yields a curve");
        // best_price is the ~3000 tip.
        assert!(depth.best_price.starts_with("29") || depth.best_price.starts_with("30"));

        let size = |p: &DepthPoint| p.trade_size.parse::<u128>().unwrap();
        let out = |p: &DepthPoint| p.output.parse::<u128>().unwrap();
        for pair in depth.points.windows(2) {
            assert!(size(&pair[1]) > size(&pair[0]), "trade size increases");
            assert!(out(&pair[1]) > out(&pair[0]), "output increases");
            assert!(pair[1].impact_pct >= pair[0].impact_pct, "impact deepens");
        }
        // The larger trades split across both makers.
        assert!(depth.points.iter().any(|p| p.makers_used == 2));

        // An unknown pair has no depth.
        assert!(svc
            .depth(&TokenPair::new(addr(1), addr(9)), Side::Sell, None)
            .is_none());
    }

    #[tokio::test]
    async fn shallow_book_truncates_the_ladder() {
        // A deep pool but a bounded payout cap: the max feasible trade only moves the price ~1%, so
        // the high-impact buckets (2%, 5%, 10%) are unreachable and the ladder stops early.
        let svc = service(
            vec![xyc(3, e(1_000_000, 18), e(3_000_000_000, 6))],
            e(30_000_000, 6),
        )
        .await;
        let pair = TokenPair::new(addr(1), addr(2));

        let depth = svc.depth(&pair, Side::Sell, None).unwrap();
        assert!(
            depth.points.len() < IMPACT_LADDER_BPS.len(),
            "a cap-limited book can't reach every bucket: {} points",
            depth.points.len()
        );
    }

    #[tokio::test]
    async fn buy_side_prices_the_reverse_direction() {
        // Buying WETH with USDC: token_in = USDC, token_out = WETH. The curve still rises and
        // deepens with size, and its tip (WETH-per-USDC) differs from the sell side (USDC-per-WETH).
        let strategies = vec![
            xyc(3, e(100, 18), e(300_000, 6)),
            xyc(4, e(200, 18), e(600_000, 6)),
        ];
        let svc = service(strategies, e(1, 30)).await;
        let pair = TokenPair::new(addr(1), addr(2));

        let buy = svc.depth(&pair, Side::Buy, None).unwrap();
        assert!(!buy.points.is_empty(), "the buy side yields a curve");
        let out = |p: &DepthPoint| p.output.parse::<u128>().unwrap();
        for w in buy.points.windows(2) {
            assert!(out(&w[1]) > out(&w[0]), "output rises");
            assert!(w[1].impact_pct >= w[0].impact_pct, "impact deepens");
        }
        let sell = svc.depth(&pair, Side::Sell, None).unwrap();
        assert_ne!(
            buy.best_price, sell.best_price,
            "the two directions price differently"
        );
    }

    #[tokio::test]
    async fn single_maker_fills_every_level_with_one_leg() {
        let svc = service(vec![xyc(3, e(100, 18), e(300_000, 6))], e(1, 30)).await;
        let pair = TokenPair::new(addr(1), addr(2));

        let depth = svc.depth(&pair, Side::Sell, None).unwrap();
        assert!(!depth.points.is_empty());
        assert!(
            depth.points.iter().all(|p| p.makers_used == 1),
            "a lone maker fills every level"
        );
    }

    #[tokio::test]
    async fn docked_only_pair_has_no_depth() {
        let svc = service(vec![docked(3, e(100, 18), e(300_000, 6))], e(1, 30)).await;
        let pair = TokenPair::new(addr(1), addr(2));
        assert!(
            svc.depth(&pair, Side::Sell, None).is_none(),
            "an inactive-only pair is not a pool"
        );
    }

    #[tokio::test]
    async fn unpriceable_pool_yields_an_empty_curve() {
        // The pair has an active strategy, so it is a pool (not 404), but nothing priceable backs
        // it — the curve is empty rather than absent.
        let svc = service(vec![unpriceable(3)], e(1, 30)).await;
        let pair = TokenPair::new(addr(1), addr(2));

        let depth = svc.depth(&pair, Side::Sell, None).unwrap();
        assert!(
            depth.points.is_empty(),
            "no priceable liquidity → no points"
        );
        assert_eq!(depth.best_price, "0");
    }
}
