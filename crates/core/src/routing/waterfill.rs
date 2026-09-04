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

use super::candidates::Candidate;

/// The solver's split: the legs to fill and the totals achieved, plus the marginal price
/// the search settled on — a warm-start seed for the next solve on this pair.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Solution {
    pub legs: Vec<RouteLeg>,
    pub amount_in: U256,
    pub amount_out: U256,
    /// Output net of per-leg gas (`amount_out − legs × per_leg_cost`); equals `amount_out`
    /// until [`solve_sparse`] charges gas.
    pub net_out: U256,
    pub lambda: Ratio,
}

/// Hard cap on bisection iterations; early-stop on the fill-gap tolerance usually converges
/// far sooner, especially from a warm λ.
const MAX_ITERS: usize = 128;
/// Cap on bracket-expansion steps when the warm λ is stale.
const BRACKET_STEPS: usize = 128;

/// The ε-optimal split of `request` across `candidates`, or `None` when the set cannot
/// meet the request even filled to its caps. `warm` is the last λ **for this same pair**
/// (λ is pair-specific); it tightens the bracket, so a good seed converges in a few
/// iterations. The result is ε-identical with or without it (both stop within tolerance).
pub fn solve(
    candidates: &[Candidate],
    request: &RouteRequest,
    warm: Option<&Ratio>,
) -> Option<Solution> {
    if candidates.is_empty() || request.amount.is_zero() {
        return None;
    }
    let bounds = input_bounds(candidates, request);
    let target = request.amount;
    let exact_in = request.exact_in;
    // Scalar fill total at a water level — allocation-free, so the bisection never touches
    // the heap (only the final assembly builds the leg vector).
    let measure = |lambda: &Ratio| measure_at(candidates, &bounds, lambda, exact_in);

    // λ=0 fills every venue to its cap. If that can't reach the target, there is no route.
    if measure(&Ratio::zero()) < target {
        return None;
    }

    // Bracket λ so `lo` over-fills (measure ≥ target) and `hi` under-fills. Seed tight around
    // the warm λ and expand only if it is stale; otherwise start wide (`lo`=0 always holds).
    let (mut lo, mut hi) = match warm.filter(|w| !w.is_zero()) {
        Some(w) => (w.clone().halved(), w.clone() + w.clone()),
        None => (Ratio::zero(), one()),
    };
    let mut m_lo = measure(&lo);
    let mut m_hi = measure(&hi);
    for _ in 0..BRACKET_STEPS {
        if m_lo >= target {
            break;
        }
        lo = lo.halved();
        m_lo = measure(&lo);
    }
    if m_lo < target {
        lo = Ratio::zero();
        m_lo = measure(&lo);
    }
    for _ in 0..BRACKET_STEPS {
        if m_hi < target {
            break;
        }
        hi = hi.clone() + hi.clone();
        m_hi = measure(&hi);
    }

    // Bisect until the fill straddles the target within tolerance (the integer touch-up in
    // assembly closes the last gap), or a hard cap.
    let tol = (target / U256::from(1_000_000u64)).max(U256::from(1u64));
    for _ in 0..MAX_ITERS {
        if m_lo.saturating_sub(m_hi) <= tol {
            break;
        }
        let mid = (lo.clone() + hi.clone()).halved();
        let m = measure(&mid);
        if m >= target {
            lo = mid;
            m_lo = m;
        } else {
            hi = mid;
            m_hi = m;
        }
    }

    // exact-in fills below target at `hi`, then adds the remainder; exact-out fills above
    // target at `lo`, then trims the overshoot (the final `quote_exact_out` confirm).
    if exact_in {
        assemble_exact_in(candidates, &bounds, &hi, target)
    } else {
        assemble_exact_out(candidates, &bounds, &lo, target)
    }
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
/// unreachable cap. An unreachable cap ⇒ the asymptote binds (`unbounded_input`).
fn input_bounds(candidates: &[Candidate], request: &RouteRequest) -> Vec<U256> {
    candidates
        .iter()
        .map(|c| {
            if request.exact_in {
                let cap = c
                    .net_quote_exact_out(c.cap_out)
                    .unwrap_or_else(|_| unbounded_input());
                cap.min(request.amount)
            } else {
                let deliverable = c.cap_out.min(request.amount);
                c.net_quote_exact_out(deliverable)
                    .unwrap_or_else(|_| unbounded_input())
            }
        })
        .collect()
}

/// Every candidate's capped net fill at water level `lambda`.
fn fills_at(candidates: &[Candidate], bounds: &[U256], lambda: &Ratio) -> Vec<LimitedQuote> {
    candidates
        .iter()
        .zip(bounds)
        .map(|(c, &bound)| {
            c.net_quote_with_limit(bound, lambda)
                .unwrap_or(LimitedQuote {
                    amount_in: U256::ZERO,
                    amount_out: U256::ZERO,
                    limited: true,
                })
        })
        .collect()
}

/// Total fill toward the target at `lambda` — input for exact-in, output for exact-out —
/// folded without allocating, so the bisection's inner loop never touches the heap.
fn measure_at(candidates: &[Candidate], bounds: &[U256], lambda: &Ratio, exact_in: bool) -> U256 {
    candidates
        .iter()
        .zip(bounds)
        .fold(U256::ZERO, |sum, (c, &bound)| {
            let fill = c
                .net_quote_with_limit(bound, lambda)
                .unwrap_or(LimitedQuote {
                    amount_in: U256::ZERO,
                    amount_out: U256::ZERO,
                    limited: true,
                });
            sum.saturating_add(if exact_in {
                fill.amount_in
            } else {
                fill.amount_out
            })
        })
}

fn one() -> Ratio {
    Ratio::from(U256::from(1u64))
}

/// Fill below `target` input at `lambda`, then place the remainder on the venue that buys
/// the most extra output for it (bounded by its box), so total input is exactly `target`.
fn assemble_exact_in(
    candidates: &[Candidate],
    bounds: &[U256],
    lambda: &Ratio,
    target: U256,
) -> Option<Solution> {
    let mut ins: Vec<U256> = fills_at(candidates, bounds, lambda)
        .iter()
        .map(|f| f.amount_in)
        .collect();
    let mut remainder =
        target.saturating_sub(ins.iter().fold(U256::ZERO, |s, &a| s.saturating_add(a)));
    while !remainder.is_zero() {
        let best = (0..candidates.len())
            .filter(|&i| bounds[i] > ins[i])
            .max_by_key(|&i| {
                let add = remainder.min(bounds[i] - ins[i]);
                let here = candidates[i]
                    .net_quote_exact_in(ins[i])
                    .unwrap_or(U256::ZERO);
                let ahead = candidates[i]
                    .net_quote_exact_in(ins[i].saturating_add(add))
                    .unwrap_or(here);
                ahead.saturating_sub(here)
            });
        let Some(i) = best else { break };
        let add = remainder.min(bounds[i] - ins[i]);
        ins[i] = ins[i].saturating_add(add);
        remainder -= add;
    }
    build(candidates, &ins, Leg::ExactIn, lambda)
}

/// Fill above `target` output at `lambda`, then trim the overshoot off the largest legs
/// so total output is exactly `target` (never under), recomputing input via exact-out.
fn assemble_exact_out(
    candidates: &[Candidate],
    bounds: &[U256],
    lambda: &Ratio,
    target: U256,
) -> Option<Solution> {
    let mut outs: Vec<U256> = fills_at(candidates, bounds, lambda)
        .iter()
        .map(|f| f.amount_out)
        .collect();
    let mut overshoot = outs
        .iter()
        .fold(U256::ZERO, |s, &o| s.saturating_add(o))
        .saturating_sub(target);
    let mut order: Vec<usize> = (0..candidates.len())
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
    build(candidates, &outs, Leg::ExactOut, lambda)
}

/// Which side of the leg the working amounts hold — the other is recomputed exactly.
enum Leg {
    ExactIn,
    ExactOut,
}

/// Turn per-candidate working amounts into legs, pricing the other side exactly.
fn build(
    candidates: &[Candidate],
    amounts: &[U256],
    side: Leg,
    lambda: &Ratio,
) -> Option<Solution> {
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
    Some(Solution {
        legs,
        amount_in,
        amount_out,
        net_out: amount_out,
        lambda: lambda.clone(),
    })
}

/// The water-fill split, then a **gas-aware sparsity pass**: repeatedly drop the smallest
/// leg and re-solve while that either raises net-of-gas output (`amount_out − legs ×
/// per_leg_cost`) or the split still exceeds `max_legs`. This is the backward-elimination
/// every production router uses (0x `reducePaths`, Balancer `optimizeSwapAmounts`) — it
/// turns the gas-free optimum's many dust legs into a few that each earn their gas.
pub fn solve_sparse(
    candidates: &[Candidate],
    request: &RouteRequest,
    per_leg_cost: U256,
    max_legs: usize,
    warm: Option<&Ratio>,
) -> Option<Solution> {
    let mut active: Vec<Candidate> = candidates.to_vec();
    let mut best = solve(&active, request, warm)?;
    while best.legs.len() > 1 {
        let net_now = net_of_gas(&best, per_leg_cost);
        let over_cap = best.legs.len() > max_legs;
        let worst = best.legs.iter().min_by_key(|l| l.amount_out)?.strategy_hash;
        let Some(pos) = active.iter().position(|c| c.key.strategy_hash == worst) else {
            break;
        };
        let removed = active.swap_remove(pos);
        // Re-solve the smaller set, warm-started from this pair's current λ (dropping one leg
        // barely moves it). Keep the drop only if it raises net-of-gas output, or we must to
        // meet `max_legs`; otherwise put the leg back and stop.
        match solve(&active, request, Some(&best.lambda)) {
            Some(next)
                if over_cap || (request.exact_in && net_of_gas(&next, per_leg_cost) >= net_now) =>
            {
                best = next;
            }
            _ => {
                active.push(removed);
                break;
            }
        }
    }
    best.net_out = net_of_gas(&best, per_leg_cost);
    Some(best)
}

/// Output net of a fixed gas charge per leg.
fn net_of_gas(sol: &Solution, per_leg_cost: U256) -> U256 {
    sol.amount_out
        .saturating_sub(per_leg_cost.saturating_mul(U256::from(sol.legs.len())))
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
        Candidate {
            key: StrategyKey {
                maker: MakerId(Address::from([hash; 20])),
                app: Address::ZERO,
                strategy_hash: StrategyHash(B256::from([hash; 32])),
            },
            token_in: tok(1),
            token_out: tok(2),
            cap_out: cap,
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
        assert_eq!(one.net_out, one.amount_out.saturating_sub(e18(100_000)));
    }
}
