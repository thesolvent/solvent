//! Capped water-fill solver: the input split across candidate venues at the
//! marginal-price optimum, subject to each venue's output cap.
//!
//! The optimum for concave (CFMM) trade functions is where every *active* venue quotes
//! the same marginal output-per-input λ (Angeris' equimarginal principle). We find λ by
//! bisection: `total(λ)` is monotone in λ, so a binary search brackets the target. This
//! is the single-pair specialization of `CFMMRouter.jl`'s dual decomposition. Fills are
//! net-of-fee ([`Candidate::net_quote_with_limit`]); the split decision runs on the
//! rescaled marginal, and the exact leg amounts are set by a final integer touch-up.

use alloy_primitives::U256;

use crate::primitives::pricing::{LimitedQuote, Ratio};
use crate::primitives::routing::{RouteLeg, RouteRequest};
use crate::primitives::MakerId;

use super::candidates::Candidate;

/// The marginal-price-optimal split of a trade across maker legs: the legs to fill, the totals
/// achieved, and the water level λ the search settled on (a warm-start seed for the next solve
/// on this pair). Gas is applied by the accessors, not stored.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Split {
    pub legs: Vec<RouteLeg>,
    pub amount_in: U256,
    pub amount_out: U256,
    pub lambda: Ratio,
}

impl Split {
    /// Output the resolver nets after gas — exact-in's take (`amount_out − legs·gas`).
    /// `per_leg_cost` is the per-leg gas in the spread token (`token_out`).
    pub fn net_output(&self, per_leg_cost: U256) -> U256 {
        self.amount_out.saturating_sub(self.gas(per_leg_cost))
    }

    /// Input the resolver grosses out with gas — exact-out's cost (`amount_in + legs·gas`).
    /// `per_leg_cost` is the per-leg gas in the spread token (`token_in`).
    pub fn gross_input(&self, per_leg_cost: U256) -> U256 {
        self.amount_in.saturating_add(self.gas(per_leg_cost))
    }

    fn gas(&self, per_leg_cost: U256) -> U256 {
        per_leg_cost.saturating_mul(U256::from(self.legs.len()))
    }
}

/// Bisection iteration caps; the fill-gap tolerance early-stops far sooner from a warm λ.
const MAX_ITERS: usize = 128;
const BRACKET_STEPS: usize = 128;

/// A leg that could not price at a level — contributes nothing.
const NO_FILL: LimitedQuote = LimitedQuote {
    amount_in: U256::ZERO,
    amount_out: U256::ZERO,
    limited: true,
};

/// The frozen inputs of one solve: the candidates, their per-leg input `bounds`, and their
/// wallet `floors`. Every fill is a pure function of a water level.
struct Fill<'a> {
    candidates: &'a [Candidate],
    bounds: Vec<U256>,
    floors: Vec<Ratio>,
    exact_in: bool,
}

impl<'a> Fill<'a> {
    fn new(candidates: &'a [Candidate], request: &RouteRequest) -> Self {
        let bounds = input_bounds(candidates, request);
        let floors = wallet_floors(candidates, &bounds);
        Self {
            candidates,
            bounds,
            floors,
            exact_in: request.exact_in,
        }
    }

    /// Leg `i`'s net fill at `level`, raised to its wallet floor; a price failure fills zero.
    fn leg(&self, i: usize, level: &Ratio) -> LimitedQuote {
        let level = if self.floors[i] > *level {
            &self.floors[i]
        } else {
            level
        };
        self.candidates[i]
            .net_quote_with_limit(self.bounds[i], level)
            .unwrap_or(NO_FILL)
    }

    /// Total toward the target at `level` — input for exact-in, output for exact-out.
    fn measure(&self, level: &Ratio) -> U256 {
        (0..self.candidates.len()).fold(U256::ZERO, |sum, i| {
            let leg = self.leg(i, level);
            let toward = if self.exact_in {
                leg.amount_in
            } else {
                leg.amount_out
            };
            sum.saturating_add(toward)
        })
    }

    /// Input the exact-in top-up may add to leg `i`: its box room, capped by its maker's
    /// remaining wallet room (zero on a price error — never the box — so no group overshoots).
    fn topup_room(&self, ins: &[U256], outs: &[U256], i: usize) -> U256 {
        let c = &self.candidates[i];
        let box_room = self.bounds[i].saturating_sub(ins[i]);
        if self.floors[i].is_zero() {
            return box_room;
        }
        let used = group_output(self.candidates, outs, c.key.maker);
        let out_room = c.wallet_cap.saturating_sub(used);
        if out_room.is_zero() {
            return U256::ZERO;
        }
        // The input to fill the maker's remaining wallet room without the round-trip
        // overshooting it; no top-up on a price failure, so the group can't exceed the wallet.
        match c.input_within_output(outs[i].saturating_add(out_room)) {
            Some(cap) => cap.saturating_sub(ins[i]).min(box_room),
            None => U256::ZERO,
        }
    }
}

/// The ε-optimal split of `request` across `candidates`, or `None` when their wallet-capped
/// capacity can't meet it. `warm` is the last λ for this pair; it only tightens the search.
pub fn solve(
    candidates: &[Candidate],
    request: &RouteRequest,
    warm: Option<&Ratio>,
) -> Option<Split> {
    if candidates.is_empty() || request.amount.is_zero() {
        return None;
    }
    let fill = Fill::new(candidates, request);
    let target = request.amount;
    // λ=0 fills every venue to its cap; if that can't reach the target, there is no route.
    if fill.measure(&Ratio::zero()) < target {
        return None;
    }
    let level = bracket_level(|l| fill.measure(l), target, warm);
    // exact-in assembles at the under-filling side then adds the remainder; exact-out at the
    // over-filling side then trims the overshoot.
    if fill.exact_in {
        assemble_exact_in(&fill, &level.hi, target)
    } else {
        assemble_exact_out(&fill, &level.lo, target)
    }
}

/// A tight bracket on the water level: `lo` still meets `target`, `hi` no longer does.
struct LevelBracket {
    lo: Ratio,
    hi: Ratio,
}

/// Bisect a `fill(level)` non-increasing in the level to a tight bracket around `target`,
/// assuming `fill(0) ≥ target` (the caller's feasibility check). `seed` (a prior level) only
/// tightens the start; the result is ε-identical without it.
fn bracket_level(
    fill: impl Fn(&Ratio) -> U256,
    target: U256,
    seed: Option<&Ratio>,
) -> LevelBracket {
    let tol = (target / U256::from(1_000_000u64)).max(U256::from(1u64));
    let mut lo = Ratio::zero();
    let mut hi = seed
        .filter(|s| !s.is_zero())
        .map(double)
        .unwrap_or_else(one);
    let mut m_lo = fill(&lo);
    let mut m_hi = fill(&hi);
    for _ in 0..BRACKET_STEPS {
        if m_hi < target {
            break;
        }
        hi = double(&hi);
        m_hi = fill(&hi);
    }
    for _ in 0..MAX_ITERS {
        if m_lo.saturating_sub(m_hi) <= tol {
            break;
        }
        let mid = midpoint(&lo, &hi);
        let m = fill(&mid);
        if m >= target {
            lo = mid;
            m_lo = m;
        } else {
            hi = mid;
            m_hi = m;
        }
    }
    LevelBracket { lo, hi }
}

/// Per-candidate wallet floor: the level at which a maker's legs' combined output hits its
/// shared wallet, so filling at `max(λ, floor)` holds the maker within budget. λ-independent,
/// hence precomputed; zero unless a maker's several strategies would over-fill it.
fn wallet_floors(candidates: &[Candidate], bounds: &[U256]) -> Vec<Ratio> {
    let mut floors = vec![Ratio::zero(); candidates.len()];
    let mut makers: Vec<MakerId> = candidates.iter().map(|c| c.key.maker).collect();
    makers.sort_unstable();
    makers.dedup();
    if makers.len() == candidates.len() {
        return floors; // every maker distinct ⇒ no shared wallet binds
    }
    for maker in makers {
        let idxs: Vec<usize> = (0..candidates.len())
            .filter(|&i| candidates[i].key.maker == maker)
            .collect();
        if idxs.len() < 2 {
            continue; // a lone leg is already within its wallet via `cap_out`
        }
        // A maker's candidates share one `WalletBudget`; bind only if full fill would exceed it.
        let wallet = candidates[idxs[0]].wallet_cap;
        if group_output_at(candidates, bounds, &idxs, &Ratio::zero()) <= wallet {
            continue;
        }
        let floor = group_floor_level(candidates, bounds, &idxs, wallet);
        for &i in &idxs {
            floors[i] = floor.clone();
        }
    }
    floors
}

/// A maker group's combined net output at water level `level`.
fn group_output_at(
    candidates: &[Candidate],
    bounds: &[U256],
    idxs: &[usize],
    level: &Ratio,
) -> U256 {
    idxs.iter().fold(U256::ZERO, |sum, &i| {
        let out = candidates[i]
            .net_quote_with_limit(bounds[i], level)
            .map(|q| q.amount_out)
            .unwrap_or(U256::ZERO);
        sum.saturating_add(out)
    })
}

/// The level at which a maker group's combined output falls to its `wallet` — where its legs
/// fill when the wallet binds. The under-budget (`hi`) side keeps the group within it.
fn group_floor_level(
    candidates: &[Candidate],
    bounds: &[U256],
    idxs: &[usize],
    wallet: U256,
) -> Ratio {
    bracket_level(
        |level| group_output_at(candidates, bounds, idxs, level),
        wallet,
        None,
    )
    .hi
}

/// An "effectively unbounded" gross input for a venue whose cap the pool can never reach
/// (e.g. an XYC cap ≥ its own reserve): large enough to approach the pool's asymptote,
/// small enough that the curve arithmetic stays within 256 bits (2^112 ≈ 5e33; realistic
/// token amounts are far smaller).
fn unbounded_input() -> U256 {
    U256::from(1u8) << 112
}

/// Per-candidate gross-input ceiling for the fill. Two constraints, whichever is tighter:
/// the box cap (`net_quote_exact_out(cap_out)`), and the request itself — no venue takes
/// more than the whole input (exact-in) or delivers more than the target (exact-out).
/// Bounding to the request keeps the numerical fill's step scaled to the trade, not to an
/// unreachable cap. Any un-priceable cap (an XYC asymptote, or arithmetic beyond the
/// fixed-point range) ⇒ the sentinel bound, which the numerical fill backs off within.
fn input_bounds(candidates: &[Candidate], request: &RouteRequest) -> Vec<U256> {
    candidates
        .iter()
        .map(|c| {
            if request.exact_in {
                c.input_within_output(c.cap_out)
                    .unwrap_or_else(unbounded_input)
                    .min(request.amount)
            } else {
                let deliverable = c.cap_out.min(request.amount);
                c.input_within_output(deliverable)
                    .unwrap_or_else(unbounded_input)
            }
        })
        .collect()
}

fn one() -> Ratio {
    Ratio::from(U256::from(1u64))
}
fn double(r: &Ratio) -> Ratio {
    r.clone() + r.clone()
}
fn midpoint(a: &Ratio, b: &Ratio) -> Ratio {
    (a.clone() + b.clone()).halved()
}

/// The saturating sum of a slice of amounts.
fn sum(amounts: &[U256]) -> U256 {
    amounts.iter().fold(U256::ZERO, |s, &a| s.saturating_add(a))
}

/// A maker's current combined output across the working `outs`.
fn group_output(candidates: &[Candidate], outs: &[U256], maker: MakerId) -> U256 {
    candidates
        .iter()
        .zip(outs)
        .filter(|(c, _)| c.key.maker == maker)
        .fold(U256::ZERO, |sum, (_, &out)| sum.saturating_add(out))
}

/// Fill below `target` input at `level`, then place the remainder on the venue that buys the
/// most extra output — bounded by its box cap and its maker's wallet — so total input is
/// `target` and no maker's combined output exceeds its wallet.
fn assemble_exact_in(fill: &Fill, level: &Ratio, target: U256) -> Option<Split> {
    let quotes: Vec<LimitedQuote> = (0..fill.candidates.len())
        .map(|i| fill.leg(i, level))
        .collect();
    let mut ins: Vec<U256> = quotes.iter().map(|q| q.amount_in).collect();
    let mut outs: Vec<U256> = quotes.iter().map(|q| q.amount_out).collect();
    let mut remainder = target.saturating_sub(sum(&ins));
    while !remainder.is_zero() {
        let Some(i) = (0..fill.candidates.len())
            .filter(|&i| !fill.topup_room(&ins, &outs, i).is_zero())
            .max_by_key(|&i| {
                let add = remainder.min(fill.topup_room(&ins, &outs, i));
                let here = fill.candidates[i]
                    .net_quote_exact_in(ins[i])
                    .unwrap_or(U256::ZERO);
                let ahead = fill.candidates[i]
                    .net_quote_exact_in(ins[i].saturating_add(add))
                    .unwrap_or(here);
                ahead.saturating_sub(here)
            })
        else {
            break;
        };
        let add = remainder.min(fill.topup_room(&ins, &outs, i));
        if add.is_zero() {
            break;
        }
        ins[i] = ins[i].saturating_add(add);
        outs[i] = fill.candidates[i]
            .net_quote_exact_in(ins[i])
            .unwrap_or(outs[i]);
        remainder -= add;
    }
    build(fill.candidates, &ins, Leg::ExactIn, level)
}

/// Fill above `target` output at `level`, then trim the overshoot off the largest legs so
/// total output is exactly `target` (never under). Trimming only lowers output, so every
/// maker stays within its wallet.
fn assemble_exact_out(fill: &Fill, level: &Ratio, target: U256) -> Option<Split> {
    let mut outs: Vec<U256> = (0..fill.candidates.len())
        .map(|i| fill.leg(i, level).amount_out)
        .collect();
    let mut overshoot = sum(&outs).saturating_sub(target);
    let mut order: Vec<usize> = (0..fill.candidates.len())
        .filter(|&i| !outs[i].is_zero())
        .collect();
    order.sort_by(|&a, &b| outs[b].cmp(&outs[a]));
    for &i in &order {
        if overshoot.is_zero() {
            break;
        }
        let cut = overshoot.min(outs[i]);
        outs[i] -= cut;
        overshoot -= cut;
    }
    build(fill.candidates, &outs, Leg::ExactOut, level)
}

/// Which side of the leg the working amounts hold — the other is recomputed exactly.
enum Leg {
    ExactIn,
    ExactOut,
}

/// Turn per-candidate working amounts into legs, pricing the other side exactly.
fn build(candidates: &[Candidate], amounts: &[U256], side: Leg, lambda: &Ratio) -> Option<Split> {
    let mut legs = Vec::new();
    let (mut amount_in, mut amount_out) = (U256::ZERO, U256::ZERO);
    for (c, &amount) in candidates.iter().zip(amounts) {
        if amount.is_zero() {
            continue;
        }
        let (leg_in, leg_out) = match side {
            Leg::ExactIn => (amount, c.net_quote_exact_in(amount).ok()?),
            Leg::ExactOut => (c.net_quote_exact_out(amount).ok()?, amount),
        };
        // A zero on either side is a dust fill the on-chain `quote()` reverts on — drop it
        // rather than build a leg the chain would reject.
        if leg_in.is_zero() || leg_out.is_zero() {
            continue;
        }
        legs.push(RouteLeg {
            maker: c.key.maker,
            strategy_hash: c.key.strategy_hash,
            token_in: c.token_in,
            token_out: c.token_out,
            amount_in: leg_in,
            amount_out: leg_out,
        });
        amount_in = amount_in.saturating_add(leg_in);
        amount_out = amount_out.saturating_add(leg_out);
    }
    if legs.is_empty() {
        return None;
    }
    Some(Split {
        legs,
        amount_in,
        amount_out,
        lambda: lambda.clone(),
    })
}

/// The water-fill split, then a **gas-aware sparsity pass**: repeatedly drop the smallest leg
/// and re-solve while that improves the resolver's take — more [`net_output`](Split::net_output)
/// for exact-in, less [`gross_input`](Split::gross_input) for exact-out — or the split still
/// exceeds `max_legs`. This is the backward-elimination every production router uses (0x
/// `reducePaths`, Balancer `optimizeSwapAmounts`): it turns the gas-free optimum's many dust
/// legs into a few that each earn their gas. `per_leg_cost` is the per-leg gas in the spread
/// token (`token_out` for exact-in, `token_in` for exact-out).
pub fn solve_sparse(
    candidates: &[Candidate],
    request: &RouteRequest,
    per_leg_cost: U256,
    max_legs: usize,
    warm: Option<&Ratio>,
) -> Option<Split> {
    let exact_in = request.exact_in;
    let mut active: Vec<Candidate> = candidates.to_vec();
    let mut best = solve(&active, request, warm)?;
    while best.legs.len() > 1 {
        let over_cap = best.legs.len() > max_legs;
        let worst = best.legs.iter().min_by_key(|l| l.amount_out)?.strategy_hash;
        let Some(pos) = active.iter().position(|c| c.key.strategy_hash == worst) else {
            break;
        };
        let removed = active.swap_remove(pos);
        // Re-solve the smaller set, warm-started from this pair's current λ (dropping one leg
        // barely moves it). Keep the drop only if it improves the resolver's take, or we must
        // to meet `max_legs`; otherwise put the leg back and stop.
        match solve(&active, request, Some(&best.lambda)) {
            Some(next)
                if over_cap
                    || (exact_in
                        && next.net_output(per_leg_cost) >= best.net_output(per_leg_cost))
                    || (!exact_in
                        && next.gross_input(per_leg_cost) <= best.gross_input(per_leg_cost)) =>
            {
                best = next;
            }
            _ => {
                active.push(removed);
                break;
            }
        }
    }
    Some(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::registry::{PeggedParams, StrategyKey};
    use crate::primitives::{IntentId, MakerId, StrategyHash};
    use crate::registry::{ConcentratePool, CurvePool, PeggedPool, XycPool};
    use alloy_primitives::{Address, B256};

    fn tok(n: u8) -> Address {
        Address::from([n; 20])
    }
    fn one18() -> U256 {
        U256::from(1_000_000_000_000_000_000u64)
    }
    fn one27() -> U256 {
        one18() * U256::from(1_000_000_000u64)
    }
    fn e18(n: u64) -> U256 {
        U256::from(n) * one18()
    }

    fn cand(hash: u8, pool: CurvePool, cap: U256, fees: &[u32]) -> Candidate {
        // Distinct maker per candidate ⇒ its own wallet group (no shared cap).
        cand_for_maker(hash, hash, pool, cap, cap, fees)
    }

    fn cand_for_maker(
        maker_byte: u8,
        hash: u8,
        pool: CurvePool,
        cap: U256,
        wallet: U256,
        fees: &[u32],
    ) -> Candidate {
        Candidate {
            key: StrategyKey {
                maker: MakerId(Address::from([maker_byte; 20])),
                app: Address::ZERO,
                strategy_hash: StrategyHash(B256::from([hash; 32])),
            },
            token_in: tok(1),
            token_out: tok(2),
            cap_out: cap,
            wallet_cap: wallet,
            pool,
            fees_in_bps: fees.to_vec(),
        }
    }

    fn xyc(reserve_in: U256, reserve_out: U256) -> CurvePool {
        CurvePool::Xyc(XycPool::from_reserves(reserve_in, reserve_out))
    }
    fn concentrate(reserve: U256) -> CurvePool {
        // A [0.5, 2.0] band around spot 1, sqrt-price in 1e18 fixed point.
        let sqrt_min = ((one18() / U256::from(2u64)) * one18()).root(2);
        let sqrt_max = (U256::from(2u64) * one18() * one18()).root(2);
        CurvePool::Concentrate(ConcentratePool::from_reserves_and_bounds(
            tok(1),
            tok(2),
            reserve,
            reserve,
            sqrt_min,
            sqrt_max,
        ))
    }
    fn pegged(reserve: U256) -> CurvePool {
        let params = PeggedParams {
            x0: reserve,
            y0: reserve,
            linear_width: U256::from(100u64) * one27(),
            rate_lt: U256::from(1u64),
            rate_gt: U256::from(1u64),
        };
        CurvePool::Pegged(PeggedPool::from_reserves_and_params(
            tok(1),
            tok(2),
            reserve,
            reserve,
            params,
        ))
    }

    fn request(amount: U256, exact_in: bool) -> RouteRequest {
        RouteRequest {
            intent: IntentId(B256::ZERO),
            token_in: tok(1),
            token_out: tok(2),
            amount,
            exact_in,
        }
    }

    /// A fine-grained greedy over the SAME net pricing — a reference the water-fill's
    /// continuous optimum must at least match (to within the chunk granularity).
    fn brute_force_out(candidates: &[Candidate], amount: U256, chunks: u64) -> U256 {
        let bounds = input_bounds(candidates, &request(amount, true));
        let mut ins = vec![U256::ZERO; candidates.len()];
        let chunk = (amount / U256::from(chunks)).max(U256::from(1u64));
        let mut remaining = amount;
        while !remaining.is_zero() {
            let step = chunk.min(remaining);
            let best = (0..candidates.len())
                .filter(|&i| bounds[i] > ins[i])
                .max_by_key(|&i| {
                    let add = step.min(bounds[i] - ins[i]);
                    let here = candidates[i]
                        .net_quote_exact_in(ins[i])
                        .unwrap_or(U256::ZERO);
                    let ahead = candidates[i]
                        .net_quote_exact_in(ins[i] + add)
                        .unwrap_or(here);
                    ahead.saturating_sub(here)
                });
            let Some(i) = best else { break };
            let add = step.min(bounds[i] - ins[i]);
            ins[i] += add;
            remaining -= add;
        }
        candidates
            .iter()
            .zip(&ins)
            .map(|(c, &a)| c.net_quote_exact_in(a).unwrap_or(U256::ZERO))
            .fold(U256::ZERO, |s, x| s + x)
    }

    #[test]
    fn exact_in_sum_equals_target_and_respects_caps() {
        let cs = [
            cand(1, xyc(e18(1000), e18(1000)), e18(1000), &[]),
            cand(2, xyc(e18(500), e18(2000)), e18(2000), &[]),
        ];
        let sol = solve(&cs, &request(e18(300), true), None).unwrap();
        assert_eq!(
            sol.amount_in,
            e18(300),
            "spends exactly the requested input"
        );
        for (leg, c) in sol.legs.iter().zip(&cs) {
            assert!(leg.amount_out <= c.cap_out, "never exceeds a venue's cap");
        }
    }

    #[test]
    fn exact_in_equalizes_marginals_across_legs() {
        // Two pools of different depth; the optimum splits across both (not all to one).
        let cs = [
            cand(1, xyc(e18(1000), e18(1000)), e18(10_000), &[]),
            cand(2, xyc(e18(3000), e18(3000)), e18(10_000), &[]),
        ];
        let sol = solve(&cs, &request(e18(400), true), None).unwrap();
        assert_eq!(sol.legs.len(), 2, "both venues are active at the optimum");
        // The deeper pool takes the larger share.
        assert!(sol.legs[1].amount_in > sol.legs[0].amount_in);
    }

    #[test]
    fn exact_in_within_epsilon_of_brute_force_heterogeneous() {
        // XYC + concentrated + pegged, mixed flat fees, one route.
        let cs = [
            cand(1, xyc(e18(2000), e18(2000)), e18(10_000), &[3_000_000]), // 0.3%
            cand(2, concentrate(e18(2000)), e18(10_000), &[]),
            cand(3, pegged(e18(2000)), e18(10_000), &[1_000_000]), // 0.1%
        ];
        let amount = e18(500);
        let sol = solve(&cs, &request(amount, true), None).unwrap();
        let brute = brute_force_out(&cs, amount, 2000);
        // The continuous optimum is at least as good as the greedy reference, within a
        // chunk of granularity.
        let eps = amount / U256::from(1000u64);
        assert!(
            sol.amount_out.saturating_add(eps) >= brute,
            "water-fill {} vs brute {}",
            sol.amount_out,
            brute
        );
        assert!(
            sol.legs.len() >= 2,
            "a heterogeneous split, not a single venue"
        );
        assert_eq!(sol.amount_in, amount);
    }

    #[test]
    fn exact_out_never_under_delivers() {
        let cs = [
            cand(1, xyc(e18(1000), e18(1000)), e18(1000), &[2_000_000]),
            cand(2, concentrate(e18(1500)), e18(1000), &[]),
        ];
        let target = e18(200);
        let sol = solve(&cs, &request(target, false), None).unwrap();
        assert_eq!(
            sol.amount_out, target,
            "delivers exactly the requested output"
        );
        assert!(sol.amount_out >= target, "never under the target");
    }

    #[test]
    fn infeasible_beyond_caps_returns_none() {
        let cs = [cand(1, xyc(e18(1000), e18(1000)), e18(10), &[])];
        // Demand far more output than the single cap can ever supply.
        assert!(solve(&cs, &request(e18(500), false), None).is_none());
    }

    #[test]
    fn warm_start_matches_cold() {
        let cs = [
            cand(1, xyc(e18(1000), e18(1000)), e18(10_000), &[]),
            cand(2, xyc(e18(2000), e18(2000)), e18(10_000), &[3_000_000]),
        ];
        let cold = solve(&cs, &request(e18(350), true), None).unwrap();
        let warm = solve(&cs, &request(e18(350), true), Some(&cold.lambda)).unwrap();
        // Exact-in spends the target exactly; the output is ε-identical (both stop within
        // the fill tolerance, not at a bit-identical λ).
        assert_eq!(cold.amount_in, warm.amount_in);
        let eps = e18(350) / U256::from(1_000_000u64);
        let diff = cold.amount_out.max(warm.amount_out) - cold.amount_out.min(warm.amount_out);
        assert!(
            diff <= eps,
            "warm {} vs cold {}",
            warm.amount_out,
            cold.amount_out
        );
    }

    #[test]
    fn sparsify_trades_legs_against_gas() {
        // Eight similar shallow pools + a trade several times a single pool's depth ⇒ the
        // gas-free optimum fragments across many of them.
        let cs: Vec<Candidate> = (1u8..=8)
            .map(|n| cand(n, xyc(e18(500), e18(500)), e18(10_000), &[]))
            .collect();
        let req = request(e18(3000), true);
        let free = solve(&cs, &req, None).unwrap();
        assert!(free.legs.len() >= 4, "gas-free optimum fragments");

        // Zero gas + a generous cap ⇒ identical to the gas-free split (no pruning).
        let unpruned = solve_sparse(&cs, &req, U256::ZERO, 100, None).unwrap();
        assert_eq!(unpruned.legs.len(), free.legs.len());

        // A hard cap alone trims to max_legs.
        let capped = solve_sparse(&cs, &req, U256::ZERO, 3, None).unwrap();
        assert!(capped.legs.len() <= 3);

        // Gas so high no second leg earns it ⇒ collapses to a single leg.
        let one = solve_sparse(&cs, &req, e18(100_000), 8, None).unwrap();
        assert_eq!(one.legs.len(), 1);
        assert_eq!(
            one.net_output(e18(100_000)),
            one.amount_out.saturating_sub(e18(100_000))
        );
    }

    #[test]
    fn sparsify_charges_gas_on_exact_out() {
        // Eight equal pools, an exact-out target one pool can meet ⇒ the gas-free optimum
        // fragments, and high per-leg gas (in input units) collapses it — exact-out now prices
        // gas via `gross_input`, where it was previously blind.
        let cs: Vec<Candidate> = (1u8..=8)
            .map(|n| cand(n, xyc(e18(2000), e18(2000)), e18(10_000), &[]))
            .collect();
        let req = request(e18(1000), false);
        let free = solve(&cs, &req, None).unwrap();
        assert!(free.legs.len() >= 2, "gas-free exact-out fragments");

        let collapsed = solve_sparse(&cs, &req, e18(100_000), 8, None).unwrap();
        assert_eq!(collapsed.legs.len(), 1, "high gas collapses exact-out");
        assert_eq!(collapsed.amount_out, e18(1000), "still delivers the target");
    }

    /// A maker's combined output across its legs.
    fn maker_out(legs: &[RouteLeg], maker_byte: u8) -> U256 {
        legs.iter()
            .filter(|l| l.maker == MakerId(Address::from([maker_byte; 20])))
            .fold(U256::ZERO, |s, l| s + l.amount_out)
    }

    #[test]
    fn shared_wallet_cap_bounds_a_makers_combined_output() {
        // Maker 1 runs two deep strategies drawing one small shared wallet; maker 2 runs one.
        let wallet = e18(100);
        let cs = [
            cand_for_maker(1, 1, xyc(e18(2000), e18(2000)), e18(10_000), wallet, &[]),
            cand_for_maker(1, 2, xyc(e18(2000), e18(2000)), e18(10_000), wallet, &[]),
            cand_for_maker(
                2,
                3,
                xyc(e18(2000), e18(2000)),
                e18(10_000),
                e18(10_000),
                &[],
            ),
        ];
        let target = e18(250);
        let sol = solve(&cs, &request(target, false), None).unwrap();
        assert_eq!(sol.amount_out, target);
        let eps = target / U256::from(1_000_000u64);
        assert!(
            maker_out(&sol.legs, 1) <= wallet + eps,
            "maker 1 combined {} exceeds its wallet {}",
            maker_out(&sol.legs, 1),
            wallet
        );

        // Lift the wallet: maker 1, uncapped, now takes more than the small wallet allowed —
        // so it was the cap, not the curve, holding its combined output down.
        let uncapped = [
            cand_for_maker(
                1,
                1,
                xyc(e18(2000), e18(2000)),
                e18(10_000),
                e18(10_000),
                &[],
            ),
            cand_for_maker(
                1,
                2,
                xyc(e18(2000), e18(2000)),
                e18(10_000),
                e18(10_000),
                &[],
            ),
            cand_for_maker(
                2,
                3,
                xyc(e18(2000), e18(2000)),
                e18(10_000),
                e18(10_000),
                &[],
            ),
        ];
        let free = solve(&uncapped, &request(target, false), None).unwrap();
        assert!(maker_out(&free.legs, 1) > wallet);
    }

    #[test]
    fn declines_when_all_liquidity_is_behind_one_over_capped_wallet() {
        // One maker, two strategies, one wallet of 100; every candidate draws it.
        let wallet = e18(100);
        let cs = [
            cand_for_maker(1, 1, xyc(e18(5000), e18(5000)), e18(10_000), wallet, &[]),
            cand_for_maker(1, 2, xyc(e18(5000), e18(5000)), e18(10_000), wallet, &[]),
        ];
        // Exact-out beyond the wallet, and exact-in whose output would exceed it, are both
        // unfillable through this set.
        assert!(solve(&cs, &request(e18(150), false), None).is_none());
        assert!(solve(&cs, &request(e18(5000), true), None).is_none());
        // Within the wallet, it fills.
        assert!(solve(&cs, &request(e18(80), false), None).is_some());
    }

    #[test]
    fn exact_in_binding_group_never_over_reserves_the_wallet() {
        // Maker 1's deep, best-priced strategies share a small wallet, so the optimum binds
        // it near its wallet with the *highest* marginal — the remainder top-up prefers it,
        // where a ceil/floor round-trip could push its combined output past the wallet. The
        // shallower maker 2 absorbs the rest.
        let wallet = e18(100);
        let cs = [
            cand_for_maker(1, 1, xyc(e18(2000), e18(2000)), e18(10_000), wallet, &[]),
            cand_for_maker(1, 2, xyc(e18(2000), e18(2000)), e18(10_000), wallet, &[]),
            cand_for_maker(2, 3, xyc(e18(500), e18(500)), e18(10_000), e18(10_000), &[]),
        ];
        let sol = solve(&cs, &request(e18(200), true), None).unwrap();
        assert_eq!(sol.amount_in, e18(200), "spends the exact input");
        assert!(
            maker_out(&sol.legs, 1) <= wallet,
            "maker 1 combined {} over-reserves its wallet {}",
            maker_out(&sol.legs, 1),
            wallet
        );
    }

    #[test]
    fn input_within_output_never_exceeds_the_cap() {
        // A 3/1000 XYC pool: net_quote_exact_out(701) = 8, net_quote_exact_in(8) = 727 > 701 —
        // filling a leg to the naive input would over-deliver past its cap and break reservation.
        let cap = U256::from(701u64);
        let c = cand(1, xyc(U256::from(3u64), U256::from(1000u64)), cap, &[]);
        let naive = c.net_quote_exact_out(cap).unwrap();
        assert!(
            c.net_quote_exact_in(naive).unwrap() > cap,
            "the naive round-trip overshoots the cap"
        );
        let bounded = c.input_within_output(cap).unwrap();
        assert!(
            c.net_quote_exact_in(bounded).unwrap() <= cap,
            "input_within_output keeps the realized output within the cap"
        );
    }
}
