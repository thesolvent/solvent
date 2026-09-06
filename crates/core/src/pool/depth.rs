//! The pool depth read-surface: the combined executable liquidity of a pair, plotted like an order
//! book the pool doesn't have. It reconstructs the book by handing the router's own machinery
//! ([`select`] + [`solve`]) a sweep of trade sizes: each size's optimal split across every active
//! maker is one point on the curve. No new curve math — depth is the split, plotted, not solved once.

use std::collections::BTreeMap;
use std::sync::Arc;

use alloy_primitives::{Address, B256, U256};

use crate::asset::AssetManager;
use crate::ledger::BudgetCache;
use crate::primitives::amount::format_units;
use crate::primitives::pool::{DepthPoint, PoolDepth, Side};
use crate::primitives::pricing::Ratio;
use crate::primitives::registry::TokenPair;
use crate::primitives::routing::RouteRequest;
use crate::primitives::{IntentId, MakerId};
use crate::registry::SharedSnapshot;
use crate::routing::{select, solve, Candidate, Split};

/// Places kept when rendering a human price string.
const PRICE_DECIMALS: u8 = 6;
/// The impact buckets the curve is anchored to (bps): 0.1% … 10%.
const IMPACT_LADDER_BPS: [u64; 6] = [10, 50, 100, 200, 500, 1000];
/// Bisection steps before a search gives up on tightening further.
const BISECT_ITERS: usize = 48;
/// The tip is priced at this fraction of capacity — small enough to sit at near-zero impact.
const PROBE_DIVISOR: u64 = 1_000_000;
/// A bisection stops once its bracket is within this fraction of its low end.
const TOLERANCE_DIVISOR: u64 = 10_000;

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
    /// active pool. Computed from the current curves + synced balances (no history, no per-request
    /// RPC); the FE zooms the returned curve client-side.
    pub fn depth(&self, pair: &TokenPair, side: Side) -> Option<PoolDepth> {
        let snapshot = self.registry.load();
        // No active pool for this pair → no depth (404).
        snapshot.active_strategies_for_pair(*pair).next()?;
        let direction = self.direction(pair, side);
        // `select` reads only the accounts it needs from the synced caps. `k = usize::MAX` keeps the
        // whole book (candidates are amount-independent — no funnel here).
        let caps = self.budgets.load();
        let candidates = select(
            &snapshot,
            &caps,
            &direction.request(U256::from(1u64)),
            usize::MAX,
        )
        .chosen;
        let axis_title = format!(
            "Cumulative {} out vs price impact",
            self.symbol(direction.token_out)
        );
        Some(
            Sweep {
                candidates: &candidates,
                direction,
            }
            .curve(axis_title),
        )
    }

    /// Which way round the pair trades for `side`, with the decimals each token renders at. `sell`
    /// sources the base out to the quote; `buy` the reverse. The curves are asymmetric.
    fn direction(&self, pair: &TokenPair, side: Side) -> Direction {
        let (base, quote) = self.assets.base_quote(pair);
        let (token_in, token_out) = match side {
            Side::Sell => (base, quote),
            Side::Buy => (quote, base),
        };
        Direction {
            token_in,
            token_out,
            in_dec: self.decimals(token_in),
            out_dec: self.decimals(token_out),
        }
    }

    fn decimals(&self, token: Address) -> u8 {
        self.assets.token(&token).map_or(18, |token| token.decimals)
    }

    fn symbol(&self, token: Address) -> String {
        self.assets
            .token(&token)
            .map_or_else(|| "token".to_string(), |token| token.symbol)
    }
}

/// One direction of one pair, with the decimals each side renders at. Carrying the two together
/// keeps them from being transposed at a call site, where the mistake prices silently.
#[derive(Clone, Copy)]
struct Direction {
    token_in: Address,
    token_out: Address,
    in_dec: u8,
    out_dec: u8,
}

impl Direction {
    /// The sweep is parameterized by **output**: `amount` is a target `token_out` amount. That makes
    /// the book's deliverable capacity the natural x-axis scale and keeps the solve well-conditioned
    /// — an exact-in size would have no bound when a maker's cap dwarfs its curve.
    fn request(&self, output: U256) -> RouteRequest {
        RouteRequest {
            intent: IntentId(B256::ZERO),
            token_in: self.token_in,
            token_out: self.token_out,
            amount: output,
            exact_in: false,
        }
    }

    /// One rung: the split's size, what it buys, and how far its blended price sits below the tip.
    fn point(&self, split: &Split, best: &Ratio) -> DepthPoint {
        DepthPoint {
            trade_size: split.amount_in.to_string(),
            output: split.amount_out.to_string(),
            effective_price: self.price(split.amount_out, split.amount_in),
            impact_pct: impact_bps(split, best) as f64 / 100.0,
            makers_used: split.legs.len() as u64,
        }
    }

    /// The human price `out/in` as a decimal string, floored to [`PRICE_DECIMALS`]. The two tokens'
    /// decimals differ, so both sides scale to a common base first.
    fn price(&self, output: U256, input: U256) -> String {
        let numerator = output
            .saturating_mul(pow10(self.in_dec))
            .saturating_mul(pow10(PRICE_DECIMALS));
        let denominator = input.saturating_mul(pow10(self.out_dec));
        Ratio::new(numerator, denominator)
            .and_then(|ratio| ratio.floor())
            .map_or_else(
                || "0".to_string(),
                |scaled| format_units(scaled, PRICE_DECIMALS),
            )
    }
}

/// The book being swept: every active venue for one direction, solved at a target output.
struct Sweep<'a> {
    candidates: &'a [Candidate],
    direction: Direction,
}

impl Sweep<'_> {
    /// Sweep the impact ladder from the tip. A book that can't price at all still answers — an
    /// empty curve, not an absent one, since the pair does have an active pool.
    fn curve(&self, axis_title: String) -> PoolDepth {
        match self.tip() {
            Some(tip) => PoolDepth {
                axis_title,
                best_price: self
                    .direction
                    .price(tip.split.amount_out, tip.split.amount_in),
                points: self.ladder(&tip),
            },
            None => PoolDepth {
                axis_title,
                best_price: "0".to_string(),
                points: Vec::new(),
            },
        }
    }

    /// The curve's anchor: the split for a probe small enough to sit at near-zero impact, and the
    /// price it quotes. An empty or unpriceable book has no tip.
    fn tip(&self) -> Option<Tip> {
        let capacity = self.capacity();
        let probe = (capacity / U256::from(PROBE_DIVISOR)).max(U256::from(1u64));
        let split = self.split(probe)?;
        let price = Ratio::new(split.amount_out, split.amount_in)?;
        Some(Tip {
            split,
            price,
            capacity,
            probe,
        })
    }

    /// One rung per impact bucket, stopping at the first the book is too shallow to reach.
    fn ladder(&self, tip: &Tip) -> Vec<DepthPoint> {
        let ceiling = self.feasible_ceiling(tip);
        let mut rungs: Vec<Split> = IMPACT_LADDER_BPS
            .into_iter()
            .map_while(|target_bps| self.size_for_impact(&tip.price, target_bps, ceiling))
            .collect();
        // A coarse book lands several buckets on one size; keep the first so the curve always rises.
        rungs.dedup_by(|later, earlier| later.amount_in <= earlier.amount_in);
        rungs
            .iter()
            .map(|split| self.direction.point(split, &tip.price))
            .collect()
    }

    /// The largest output the book actually delivers. Caps bound the payout but the curves need not
    /// attain them — an XYC's cap sits on its own asymptote, and a maker's shared wallet binds its
    /// strategies together — so search for the ceiling rather than assume one, and every rung below
    /// stays fillable.
    fn feasible_ceiling(&self, tip: &Tip) -> U256 {
        match self.fills(tip.capacity) {
            true => tip.capacity,
            false => {
                Bracket::new(tip.probe, tip.capacity)
                    .narrow(|output| self.fills(output))
                    .lo
            }
        }
    }

    /// The smallest output whose blended price impact reaches `target_bps`, or `None` when even the
    /// whole book stays under it. Impact is monotone in the output for a concave book, so `lo` never
    /// reaches the bucket, `hi` always does, and `hi` converges onto the crossing.
    fn size_for_impact(&self, best: &Ratio, target_bps: u64, ceiling: U256) -> Option<Split> {
        let reaches = |output: U256| {
            self.split(output)
                .is_some_and(|split| impact_bps(&split, best) >= target_bps)
        };
        match reaches(ceiling) {
            false => None,
            true => self.split(
                Bracket::new(U256::from(1u64), ceiling)
                    .narrow(|output| !reaches(output))
                    .hi,
            ),
        }
    }

    /// The book's deliverable output. A maker's strategies draw one shared wallet, so its several
    /// candidates cannot each pay out their own cap — the maker contributes at most that wallet.
    fn capacity(&self) -> U256 {
        let mut by_maker: BTreeMap<MakerId, MakerCap> = BTreeMap::new();
        for candidate in self.candidates {
            let maker = by_maker
                .entry(candidate.key.maker)
                .or_insert_with(|| MakerCap::new(candidate.wallet_cap));
            maker.legs = maker.legs.saturating_add(candidate.cap_out);
        }
        by_maker.values().fold(U256::ZERO, |sum, maker| {
            sum.saturating_add(maker.deliverable())
        })
    }

    /// The optimal split delivering exactly `output`, or `None` when the book cannot reach it.
    fn split(&self, output: U256) -> Option<Split> {
        solve(self.candidates, &self.direction.request(output), None)
    }

    fn fills(&self, output: U256) -> bool {
        self.split(output).is_some()
    }
}

/// The curve's anchor, plus the sweep scale it was measured against.
struct Tip {
    split: Split,
    price: Ratio,
    capacity: U256,
    probe: U256,
}

/// One maker's two output ceilings: the wallet its strategies share, and what its legs could each
/// deliver alone.
struct MakerCap {
    wallet: U256,
    legs: U256,
}

impl MakerCap {
    fn new(wallet: U256) -> Self {
        Self {
            wallet,
            legs: U256::ZERO,
        }
    }

    fn deliverable(&self) -> U256 {
        self.wallet.min(self.legs)
    }
}

/// A bisection bracket over trade sizes.
struct Bracket {
    lo: U256,
    hi: U256,
}

impl Bracket {
    fn new(lo: U256, hi: U256) -> Self {
        Self { lo, hi }
    }

    /// Bisect until tight, raising `lo` onto every midpoint that `holds` and lowering `hi` onto
    /// every one that doesn't.
    fn narrow(mut self, mut holds: impl FnMut(U256) -> bool) -> Self {
        let mut remaining = BISECT_ITERS;
        while remaining > 0 && !self.is_tight() {
            remaining -= 1;
            let mid = self.midpoint();
            match holds(mid) {
                true => self.lo = mid,
                false => self.hi = mid,
            }
        }
        self
    }

    /// Within [`TOLERANCE_DIVISOR`] of the low end. The ladder is plotted, not settled, so wei-exact
    /// rungs would buy nothing but solver calls; the one-wei floor keeps tiny brackets terminating.
    fn is_tight(&self) -> bool {
        self.hi.saturating_sub(self.lo)
            <= (self.lo / U256::from(TOLERANCE_DIVISOR)).max(U256::from(1u64))
    }

    fn midpoint(&self) -> U256 {
        self.lo
            .saturating_add(self.hi.saturating_sub(self.lo) / U256::from(2u64))
    }
}

/// How far a split's blended price sits from the tip, in bps. A split that can't price is at the
/// tip by definition — it moved nothing.
fn impact_bps(split: &Split, best: &Ratio) -> u64 {
    Ratio::new(split.amount_out, split.amount_in)
        .map_or(0, |effective| effective.rel_diff_bps(best))
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
    use crate::primitives::registry::{
        Curve, CurveSpec, MakerStrategy, PeggedParams, Snapshot, StrategyKey,
    };
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

    /// A strategy over WETH(2)/USDC(1) on `curve`, keyed to a maker of its own.
    fn strategy(hash: u8, curve: Curve, weth: U256, usdc: U256) -> MakerStrategy {
        let key = StrategyKey {
            maker: MakerId(addr(hash)),
            app: Address::ZERO,
            strategy_hash: StrategyHash(B256::from([hash; 32])),
        };
        let mut strategy = MakerStrategy::new(key, &[]);
        strategy.curve = CurveSpec::Priceable {
            curve,
            fees_in_bps: vec![],
        };
        strategy.balances.insert(addr(WETH), weth); // token_in
        strategy.balances.insert(addr(USDC), usdc); // token_out (payout)
        strategy
    }

    /// An XYC strategy over WETH(2)/USDC(1) with the given raw reserves.
    fn xyc(hash: u8, weth: U256, usdc: U256) -> MakerStrategy {
        strategy(hash, Curve::Xyc, weth, usdc)
    }

    /// √-price of the canonical direction — the higher-address token per the lower one, in *raw*
    /// units, so WETH-wei per USDC-wei — for a 100 WETH / 300k USDC pool: √(1e20/3e11) ≈ 18257.4,
    /// in the program's 1e18 fixed point.
    fn sqrt_price_3000() -> U256 {
        U256::from(18_257_418_583_505_537u64) * U256::from(1_000_000u64)
    }

    /// A concentrated strategy banded `±spread_bps` (in √-price) around that anchor. The band
    /// amplifies: narrow bands grow the virtual reserves, so the same real reserves quote flatter.
    fn concentrated(hash: u8, weth: U256, usdc: U256, spread_bps: u64) -> MakerStrategy {
        let anchor = sqrt_price_3000();
        let bps = U256::from(10_000u64);
        let spread = U256::from(10_000u64 + spread_bps);
        strategy(
            hash,
            Curve::Concentrate {
                sqrt_price_min: anchor * bps / spread,
                sqrt_price_max: anchor * spread / bps,
            },
            weth,
            usdc,
        )
    }

    /// A pegged (stableswap) strategy pegged at ~3000 USDC/WETH: the rates scale each token to a
    /// common base and the normalization factors put the reserves on the peg. `off_peg` scales the
    /// WETH-side factor, moving them off it — where the two directions stop being symmetric.
    fn pegged(hash: u8, weth: U256, usdc: U256, off_peg: u64) -> MakerStrategy {
        const RATE_USDC: u64 = 1_000_000_000;
        const RATE_WETH: u64 = 3;
        let params = PeggedParams {
            x0: usdc * U256::from(RATE_USDC),
            y0: weth * U256::from(RATE_WETH) * U256::from(off_peg),
            linear_width: U256::from(100u64) * U256::from(10u64).pow(U256::from(27u64)),
            rate_lt: U256::from(RATE_USDC),
            rate_gt: U256::from(RATE_WETH),
        };
        strategy(hash, Curve::Pegged(params), weth, usdc)
    }

    /// The same strategy charging a flat input fee (`bps` at the 1e9 denominator).
    fn with_fee(mut strategy: MakerStrategy, bps: u32) -> MakerStrategy {
        if let CurveSpec::Priceable { fees_in_bps, .. } = &mut strategy.curve {
            fees_in_bps.push(bps);
        }
        strategy
    }

    /// The same strategy re-keyed to `maker` — two of these share one wallet budget.
    fn for_maker(mut strategy: MakerStrategy, maker: u8) -> MakerStrategy {
        strategy.key.maker = MakerId(addr(maker));
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

    const USDC: u8 = 1;
    const WETH: u8 = 2;

    /// The traded pair. `TokenPair` is unordered, so this is the same value either way round.
    fn pair() -> TokenPair {
        TokenPair::new(addr(USDC), addr(WETH))
    }

    /// Mirrors the real source: a strategy virtual is its registry balance, a wallet is the maker's
    /// on-chain pullable. So the deliverable cap = `min(wallet, registry balance)` — a huge wallet
    /// lets the curve (reserve) bind; a small one caps the book below it.
    struct FakeBudget {
        registry: Arc<SharedSnapshot>,
        wallets: BTreeMap<(MakerId, Address), U256>,
        default_wallet: U256,
    }

    #[async_trait::async_trait]
    impl BudgetSource for FakeBudget {
        async fn budget(&self, account: &AccountKey) -> Result<U256, BudgetSourceError> {
            match account {
                AccountKey::WalletBudget { maker, token } => Ok(self
                    .wallets
                    .get(&(*maker, *token))
                    .copied()
                    .unwrap_or(self.default_wallet)),
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

    /// A test book: the strategies, each maker's on-chain pullable (per token, defaulting to a
    /// wallet that never binds), and the token catalog.
    struct Fixture {
        strategies: Vec<MakerStrategy>,
        wallets: BTreeMap<(MakerId, Address), U256>,
        default_wallet: U256,
        tokens: Vec<TokenMeta>,
    }

    impl Fixture {
        fn new(strategies: Vec<MakerStrategy>) -> Self {
            Self {
                strategies,
                wallets: BTreeMap::new(),
                default_wallet: e(1, 30),
                tokens: vec![meta(USDC, "USDC", 6, true), meta(WETH, "WETH", 18, false)],
            }
        }

        /// Pin one maker's pullable for one token — the on-chain leg of `min(wallet, virtual)`.
        fn wallet(mut self, maker: u8, token: u8, amount: U256) -> Self {
            self.wallets
                .insert((MakerId(addr(maker)), addr(token)), amount);
            self
        }

        fn tokens(mut self, tokens: Vec<TokenMeta>) -> Self {
            self.tokens = tokens;
            self
        }

        /// Build the service with its caps already synced, as a warm server would have them.
        async fn build(self) -> Harness {
            let harness = self.build_cold();
            harness.budgets.refresh().await.unwrap();
            harness
        }

        /// Build with the cache left cold — nothing synced, every cap zero.
        fn build_cold(self) -> Harness {
            let list = TokenList {
                name: "test".to_string(),
                tokens: self.tokens,
            };
            let registry = Arc::new(SharedSnapshot::new(Snapshot::from_strategies(
                self.strategies,
            )));
            let assets = Arc::new(AssetManager::new(list, Arc::clone(&registry)));
            let source: Arc<dyn BudgetSource> = Arc::new(FakeBudget {
                registry: Arc::clone(&registry),
                wallets: self.wallets,
                default_wallet: self.default_wallet,
            });
            let budgets = Arc::new(BudgetCache::new(source, Arc::clone(&registry)));
            Harness {
                depth: DepthService::new(Arc::clone(&registry), Arc::clone(&budgets), assets),
                registry,
                budgets,
            }
        }
    }

    /// A built book, keeping the registry and cache reachable so a test can re-sync them.
    struct Harness {
        depth: DepthService,
        registry: Arc<SharedSnapshot>,
        budgets: Arc<BudgetCache>,
    }

    impl Harness {
        fn sell(&self) -> PoolDepth {
            self.depth.depth(&pair(), Side::Sell).unwrap()
        }

        fn buy(&self) -> PoolDepth {
            self.depth.depth(&pair(), Side::Buy).unwrap()
        }
    }

    /// The book with one uniform wallet across every maker.
    async fn service(strategies: Vec<MakerStrategy>, cap: U256) -> DepthService {
        let mut fixture = Fixture::new(strategies);
        fixture.default_wallet = cap;
        fixture.build().await.depth
    }

    /// Every point's trade size, for comparing two books rung by rung.
    fn sizes(depth: &PoolDepth) -> Vec<u128> {
        depth
            .points
            .iter()
            .map(|p| p.trade_size.parse().unwrap())
            .collect()
    }

    /// A curve that only ever rises: strictly increasing size and output, never-decreasing impact.
    fn assert_rising(depth: &PoolDepth) {
        for window in depth.points.windows(2) {
            let (lo, hi) = (&window[0], &window[1]);
            assert!(
                hi.trade_size.parse::<u128>().unwrap() > lo.trade_size.parse::<u128>().unwrap(),
                "trade size must strictly increase: {} then {}",
                lo.trade_size,
                hi.trade_size
            );
            assert!(
                hi.output.parse::<u128>().unwrap() > lo.output.parse::<u128>().unwrap(),
                "output must strictly increase: {} then {}",
                lo.output,
                hi.output
            );
            assert!(hi.impact_pct >= lo.impact_pct, "impact must deepen");
        }
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

        let depth = svc.depth(&pair, Side::Sell).unwrap();
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
            .depth(&TokenPair::new(addr(1), addr(9)), Side::Sell)
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

        let depth = svc.depth(&pair, Side::Sell).unwrap();
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

        let buy = svc.depth(&pair, Side::Buy).unwrap();
        assert!(!buy.points.is_empty(), "the buy side yields a curve");
        let out = |p: &DepthPoint| p.output.parse::<u128>().unwrap();
        for w in buy.points.windows(2) {
            assert!(out(&w[1]) > out(&w[0]), "output rises");
            assert!(w[1].impact_pct >= w[0].impact_pct, "impact deepens");
        }
        let sell = svc.depth(&pair, Side::Sell).unwrap();
        assert_ne!(
            buy.best_price, sell.best_price,
            "the two directions price differently"
        );
    }

    #[tokio::test]
    async fn single_maker_fills_every_level_with_one_leg() {
        let svc = service(vec![xyc(3, e(100, 18), e(300_000, 6))], e(1, 30)).await;
        let pair = TokenPair::new(addr(1), addr(2));

        let depth = svc.depth(&pair, Side::Sell).unwrap();
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
            svc.depth(&pair, Side::Sell).is_none(),
            "an inactive-only pair is not a pool"
        );
    }

    #[tokio::test]
    async fn unpriceable_pool_yields_an_empty_curve() {
        // The pair has an active strategy, so it is a pool (not 404), but nothing priceable backs
        // it — the curve is empty rather than absent.
        let svc = service(vec![unpriceable(3)], e(1, 30)).await;
        let pair = TokenPair::new(addr(1), addr(2));

        let depth = svc.depth(&pair, Side::Sell).unwrap();
        assert!(
            depth.points.is_empty(),
            "no priceable liquidity → no points"
        );
        assert_eq!(depth.best_price, "0");
    }

    #[tokio::test]
    async fn shared_wallet_maker_still_yields_a_ladder() {
        // One maker, two deep strategies, one wallet: each strategy's cap is that whole wallet, so
        // summing caps counts a payout the maker can only make once.
        let wallet = e(50_000, 6);
        let book = Fixture::new(vec![
            for_maker(xyc(3, e(100, 18), e(300_000, 6)), 9),
            for_maker(xyc(4, e(200, 18), e(600_000, 6)), 9),
        ])
        .wallet(9, USDC, wallet)
        .build()
        .await;

        let depth = book.sell();
        assert!(!depth.points.is_empty(), "a deep maker is not zero depth");
        assert_rising(&depth);
        let top: u128 = depth.points.last().unwrap().output.parse().unwrap();
        assert!(
            U256::from(top) <= wallet,
            "the shared wallet bounds the book: {top} > {wallet}"
        );
    }

    #[tokio::test]
    async fn high_supply_reserves_still_yield_a_ladder() {
        // A ten-trillion-supply token. Past ~5e30 the solver falls back to its unbounded-input
        // sentinel, which stops just short of the caps — an assumed ceiling would be unfillable.
        let book = Fixture::new(vec![
            xyc(3, e(10_000_000_000_000, 18), e(300_000, 6)),
            xyc(4, e(20_000_000_000_000, 18), e(600_000, 6)),
        ])
        .build()
        .await;

        let depth = book.sell();
        assert!(!depth.points.is_empty(), "a deep book is not zero depth");
        assert_rising(&depth);
    }

    #[tokio::test]
    async fn every_point_reaches_at_least_its_bucket() {
        // Each rung is the smallest size whose impact reaches its bucket, so a rung never reports
        // less. A coarse book crosses several buckets in one wei and must still rise rung by rung.
        let smooth = vec![
            xyc(3, e(100, 18), e(300_000, 6)),
            xyc(4, e(200, 18), e(600_000, 6)),
        ];
        let coarse = vec![xyc(3, U256::from(3u64), U256::from(1000u64))];
        for strategies in [smooth, coarse] {
            let depth = Fixture::new(strategies).build().await.sell();
            assert_rising(&depth);
            assert!(depth.points.len() <= IMPACT_LADDER_BPS.len());
            for (point, bucket) in depth.points.iter().zip(IMPACT_LADDER_BPS) {
                assert!(
                    point.impact_pct >= bucket as f64 / 100.0,
                    "{}% sits under its {bucket} bps bucket",
                    point.impact_pct
                );
            }
        }
    }

    #[tokio::test]
    async fn a_fee_shifts_the_tip_not_the_shape() {
        // A flat fee is a level shift. It moves the tip down by the fee, and because impact is
        // measured against that same net tip, the ladder lands on nearly the same sizes.
        const FEE_BPS: u32 = 3_000_000; // 0.3% at the 1e9 denominator
        let reserves = || xyc(3, e(100, 18), e(300_000, 6));
        let free = Fixture::new(vec![reserves()]).build().await.sell();
        let charged = Fixture::new(vec![with_fee(reserves(), FEE_BPS)])
            .build()
            .await
            .sell();

        let tip = |depth: &PoolDepth| depth.best_price.parse::<f64>().unwrap();
        let shift_bps = (tip(&free) - tip(&charged)) / tip(&free) * 10_000.0;
        assert!(
            (shift_bps - 30.0).abs() < 1.0,
            "a 0.3% fee moves the tip ~30 bps, got {shift_bps}"
        );
        assert_eq!(
            free.points.len(),
            charged.points.len(),
            "a fee-free tip would swallow the shallowest bucket"
        );
        for (plain, fee) in sizes(&free).iter().zip(sizes(&charged)) {
            let drift = (*plain as f64 - fee as f64).abs() / *plain as f64;
            assert!(
                drift < 0.05,
                "the fee reshaped the ladder: {plain} vs {fee}"
            );
        }
    }

    #[tokio::test]
    async fn zero_pullable_maker_is_not_advertised() {
        // A revoked allowance is not liquidity: the maker leaves the book rather than showing as
        // depth no taker could ever pull.
        let live = || xyc(3, e(100, 18), e(300_000, 6));
        let alone = Fixture::new(vec![live()]).build().await.sell();
        let with_revoked = Fixture::new(vec![live(), xyc(4, e(200, 18), e(600_000, 6))])
            .wallet(4, USDC, U256::ZERO)
            .build()
            .await
            .sell();

        assert!(with_revoked.points.iter().all(|p| p.makers_used == 1));
        assert_eq!(
            sizes(&alone),
            sizes(&with_revoked),
            "a revoked maker adds no depth"
        );
        assert_eq!(alone.best_price, with_revoked.best_price);
    }

    #[tokio::test]
    async fn pullable_binds_below_the_curve() {
        // The cap is min(wallet, virtual): squeezing one maker's wallet shrinks the book even
        // though its curve and registry balance are untouched.
        let strategies = || {
            vec![
                xyc(3, e(100, 18), e(300_000, 6)),
                xyc(4, e(200, 18), e(600_000, 6)),
            ]
        };
        let open = Fixture::new(strategies()).build().await.sell();
        let squeezed = Fixture::new(strategies())
            .wallet(3, USDC, e(10_000, 6))
            .build()
            .await
            .sell();

        let top = |depth: &PoolDepth| depth.points.last().unwrap().output.parse::<u128>().unwrap();
        assert_rising(&squeezed);
        assert!(
            top(&squeezed) < top(&open),
            "a tighter wallet is a smaller book: {} vs {}",
            top(&squeezed),
            top(&open)
        );
    }

    #[tokio::test]
    async fn narrow_band_concentrate_is_deeper_than_xyc() {
        // The band amplifies: the same real reserves banded tightly grow into much larger virtual
        // ones, so every impact bucket takes a bigger trade to reach.
        let plain = Fixture::new(vec![xyc(3, e(100, 18), e(300_000, 6))])
            .build()
            .await
            .sell();
        let banded = Fixture::new(vec![concentrated(3, e(100, 18), e(300_000, 6), 100)])
            .build()
            .await
            .sell();

        assert_rising(&banded);
        for (flat, deep) in sizes(&plain).iter().zip(sizes(&banded)) {
            assert!(
                deep > *flat,
                "the banded book should absorb more at the same impact: {flat} vs {deep}"
            );
        }
    }

    #[tokio::test]
    async fn wide_band_concentrate_degenerates_to_xyc() {
        // Widen the band far enough and the growth terms vanish — concentrate *is* XYC on its
        // virtual reserves, so the two curves converge.
        let plain = Fixture::new(vec![xyc(3, e(100, 18), e(300_000, 6))])
            .build()
            .await
            .sell();
        let banded = Fixture::new(vec![concentrated(
            3,
            e(100, 18),
            e(300_000, 6),
            100_000_000,
        )])
        .build()
        .await
        .sell();

        let tip = |depth: &PoolDepth| depth.best_price.parse::<f64>().unwrap();
        assert!(
            (tip(&plain) - tip(&banded)).abs() / tip(&plain) < 1e-6,
            "the tips converge: {} vs {}",
            plain.best_price,
            banded.best_price
        );
        assert_eq!(sizes(&plain).len(), sizes(&banded).len());
        for (flat, wide) in sizes(&plain).iter().zip(sizes(&banded)) {
            let drift = (*flat as f64 - wide as f64).abs() / *flat as f64;
            assert!(drift < 0.01, "a wide band is XYC: {flat} vs {wide}");
        }
    }

    #[tokio::test]
    async fn concentrate_delivers_its_full_cap_where_xyc_cannot() {
        // The band's edge is exactly where a real reserve empties, and the cap is a slice of the
        // grown virtual reserve — so a banded pool prices its whole payout and the ladder stops on
        // the cap. An XYC's cap sits on its own asymptote, so its ladder never approaches it.
        let cap = e(300_000, 6);
        let banded = Fixture::new(vec![concentrated(3, e(100, 18), cap, 100)])
            .build()
            .await
            .sell();
        let plain = Fixture::new(vec![xyc(3, e(100, 18), cap)])
            .build()
            .await
            .sell();

        let reach = |depth: &PoolDepth| {
            let top: u128 = depth.points.last().unwrap().output.parse().unwrap();
            top as f64 / 300_000e6
        };
        assert!(
            reach(&banded) > reach(&plain) * 5.0,
            "banded reached {} of its cap, plain {}",
            reach(&banded),
            reach(&plain)
        );
        assert!(
            banded.points.len() < plain.points.len(),
            "the band edge truncates the ladder where XYC keeps going"
        );
    }

    #[tokio::test]
    async fn mixed_curve_book_splits_across_all_three() {
        // XYC, concentrated and pegged quoting one pair. The XYC is priced well above the other
        // two, so the ladder starts on it alone and pulls them in as the trade walks the price
        // down — the venue set only ever grows.
        let book = Fixture::new(vec![
            xyc(3, e(100, 18), e(310_000, 6)),
            concentrated(4, e(100, 18), e(300_000, 6), 5_000),
            pegged(5, e(100, 18), e(300_000, 6), 1),
        ])
        .build()
        .await;

        let depth = book.sell();
        assert_rising(&depth);
        assert_eq!(
            depth.points.first().unwrap().makers_used,
            1,
            "the best-priced venue takes the first rung alone"
        );
        for window in depth.points.windows(2) {
            assert!(
                window[1].makers_used >= window[0].makers_used,
                "a venue never drops out as the trade grows"
            );
        }
        assert!(
            depth.points.iter().any(|p| p.makers_used == 3),
            "a deep rung uses the whole book"
        );
    }

    #[tokio::test]
    async fn pegged_off_peg_prices_the_two_sides_differently() {
        // A stableswap is symmetric at its peg, so both directions climb the ladder in the same
        // shape. Move the reserves off it and one side is deep where the other is shallow.
        let shape = |depth: &PoolDepth| {
            let rungs = sizes(depth);
            *rungs.last().unwrap() as f64 / *rungs.first().unwrap() as f64
        };
        let at_peg = Fixture::new(vec![pegged(3, e(100, 18), e(300_000, 6), 1)])
            .build()
            .await;
        let skewed = Fixture::new(vec![pegged(3, e(100, 18), e(300_000, 6), 2)])
            .build()
            .await;

        let balanced = (shape(&at_peg.sell()) - shape(&at_peg.buy())).abs() / shape(&at_peg.sell());
        let tilted = (shape(&skewed.sell()) - shape(&skewed.buy())).abs() / shape(&skewed.sell());
        assert!(
            balanced < 0.05,
            "at the peg the two sides match: {balanced}"
        );
        assert!(tilted > balanced, "off the peg they should not: {tilted}");
    }

    #[tokio::test]
    async fn cold_cache_reports_a_pool_with_no_depth() {
        // Depth reads only the synced snapshot. Before the first tick nothing is pullable, so a
        // live pool reports no depth rather than a 404 or a panic — and a registry change stays
        // invisible until the next sync.
        let book = Fixture::new(vec![xyc(3, e(100, 18), e(300_000, 6))]).build_cold();

        let cold = book.sell();
        assert!(cold.points.is_empty(), "nothing is pullable before a sync");
        assert_eq!(cold.best_price, "0");

        book.budgets.refresh().await.unwrap();
        let warm = book.sell();
        assert!(!warm.points.is_empty());

        book.registry.store(Snapshot::from_strategies(vec![
            xyc(3, e(100, 18), e(300_000, 6)),
            xyc(4, e(200, 18), e(600_000, 6)),
        ]));
        assert_eq!(
            sizes(&book.sell()),
            sizes(&warm),
            "stale caps hide the new maker"
        );

        book.budgets.refresh().await.unwrap();
        let top = |depth: &PoolDepth| depth.points.last().unwrap().output.parse::<u128>().unwrap();
        assert!(
            top(&book.sell()) > top(&warm),
            "the sync brings the new maker in"
        );
    }

    #[tokio::test]
    async fn rendered_price_matches_the_exact_ratio() {
        // Both sides scale to a common base before flooring, so the rendered price has to agree
        // with the raw amounts whatever the pair's decimals.
        let layouts = [
            (6u8, e(300_000, 6)),
            (18, e(300_000, 18)),
            (0, U256::from(300_000u64)),
        ];
        for (out_dec, reserve) in layouts {
            let depth = Fixture::new(vec![xyc(3, e(100, 18), reserve)])
                .tokens(vec![
                    meta(USDC, "USDC", out_dec, true),
                    meta(WETH, "WETH", 18, false),
                ])
                .build()
                .await
                .sell();

            assert!(
                !depth.points.is_empty(),
                "{out_dec} decimals priced nothing"
            );
            for point in &depth.points {
                let input = point.trade_size.parse::<f64>().unwrap();
                let output = point.output.parse::<f64>().unwrap();
                let expected = (output / 10f64.powi(i32::from(out_dec))) / (input / 1e18);
                let rendered = point.effective_price.parse::<f64>().unwrap();
                assert!(
                    (rendered - expected).abs() / expected < 1e-6,
                    "{out_dec} decimals rendered {rendered}, expected {expected}"
                );
            }
        }
    }

    #[tokio::test]
    async fn query_order_does_not_change_the_curve_side_does() {
        // The pair is unordered and the catalog decides orientation, so naming it either way round
        // cannot change the answer. Only `side` can.
        let book = Fixture::new(vec![xyc(3, e(100, 18), e(300_000, 6))])
            .build()
            .await;
        let one_way = book
            .depth
            .depth(&TokenPair::new(addr(USDC), addr(WETH)), Side::Sell)
            .unwrap();
        let other_way = book
            .depth
            .depth(&TokenPair::new(addr(WETH), addr(USDC)), Side::Sell)
            .unwrap();

        assert_eq!(sizes(&one_way), sizes(&other_way));
        assert_eq!(one_way.best_price, other_way.best_price);
        assert_ne!(
            one_way.best_price,
            book.buy().best_price,
            "side does change it"
        );
    }

    #[tokio::test]
    async fn depth_is_byte_identical_across_runs() {
        // Ties break on strategy_hash the whole way down, so the same book renders the same curve
        // every call — a chart that flickers between requests is a bug.
        let book = Fixture::new(vec![
            xyc(3, e(100, 18), e(300_000, 6)),
            xyc(4, e(100, 18), e(300_000, 6)),
            xyc(5, e(100, 18), e(300_000, 6)),
            concentrated(6, e(100, 18), e(300_000, 6), 5_000),
        ])
        .build()
        .await;

        let first = serde_json::to_string(&book.sell()).unwrap();
        let second = serde_json::to_string(&book.sell()).unwrap();
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn dust_book_yields_a_tip_and_no_ladder() {
        // A few wei of payout: the probe and the ceiling collapse onto one size, so there is a tip
        // but nothing to plot — and nothing divides by zero on the way.
        let depth = Fixture::new(vec![xyc(3, e(100, 18), e(300_000, 6))])
            .wallet(3, USDC, U256::from(5u64))
            .build()
            .await
            .sell();

        let tip = depth.best_price.parse::<f64>().unwrap();
        assert!(
            (tip - 3000.0).abs() < 1.0,
            "the tip still prices: {}",
            depth.best_price
        );
        assert!(
            depth.points.is_empty(),
            "5 wei of payout is not a curve: {:?}",
            depth.points
        );
    }
}
