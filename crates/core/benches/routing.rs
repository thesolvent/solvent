//! Routing `solve_sparse` latency across funnel sizes K. The shipped funnel is K ≤ 8 (research:
//! production routers use 3–7 sources); larger K is timed only to show the scaling that motivates
//! keeping it small. Plain `Instant` timing — no criterion. Release-built via the `bench` profile:
//! `cargo bench -p solvent-core`. Exits non-zero if p99 exceeds the 500 ms budget at a shipped K
//! (≤ 8). Run with `--features quote-metrics` to also print curve-quote requests per solve — the
//! `latency ≈ Σ quote-calls × per-curve unit cost` model behind the K sizing.

use std::hint::black_box;
use std::io::{stdout, Write};
use std::time::Instant;

use alloy_primitives::{Address, B256, U256};

use solvent_core::primitives::registry::{PeggedParams, StrategyKey};
use solvent_core::primitives::routing::RouteRequest;
use solvent_core::primitives::{IntentId, MakerId, StrategyHash};
use solvent_core::registry::{ConcentratePool, CurvePool, PeggedPool, XycPool};
#[cfg(feature = "quote-metrics")]
use solvent_core::routing::{quote_calls, reset_quote_calls};
use solvent_core::routing::{solve_sparse, Candidate};

const BUDGET_US: u128 = 500_000; // 500 ms

fn e18(n: u64) -> U256 {
    U256::from(n) * U256::from(10u64).pow(U256::from(18u64))
}
fn one27() -> U256 {
    U256::from(10u64).pow(U256::from(27u64))
}
fn tok(n: u8) -> Address {
    Address::from([n; 20])
}

/// A `k`-candidate funnel: XYC / concentrated / pegged in rotation, varied depths, ~half with
/// a flat fee — built through the public API, like the routing study.
fn funnel(k: u32) -> Vec<Candidate> {
    let sqrt_min = ((e18(1) / U256::from(2u64)) * e18(1)).root(2);
    let sqrt_max = (U256::from(2u64) * e18(1) * e18(1)).root(2);
    (0..k)
        .map(|id| {
            let depth = e18(500 + u64::from(id % 40) * 100);
            let pool = match id % 3 {
                0 => CurvePool::Xyc(XycPool::from_reserves(depth, depth)),
                1 => CurvePool::Concentrate(ConcentratePool::from_reserves_and_bounds(
                    tok(1),
                    tok(2),
                    depth,
                    depth,
                    sqrt_min,
                    sqrt_max,
                )),
                _ => CurvePool::Pegged(PeggedPool::from_reserves_and_params(
                    tok(1),
                    tok(2),
                    depth,
                    depth,
                    PeggedParams {
                        x0: depth,
                        y0: depth,
                        linear_width: U256::from(100u64) * one27(),
                        rate_lt: U256::from(1u64),
                        rate_gt: U256::from(1u64),
                    },
                )),
            };
            let fees = if id % 2 == 0 { vec![3_000_000] } else { vec![] };
            let mut hash = [0u8; 32];
            hash[28..].copy_from_slice(&id.to_be_bytes());
            let key = StrategyKey {
                maker: MakerId(Address::from([(id % 251 + 1) as u8; 20])),
                app: Address::ZERO,
                strategy_hash: StrategyHash(B256::from(hash)),
            };
            Candidate::new(key, tok(1), tok(2), e18(10_000), U256::MAX, pool, fees)
        })
        .collect()
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

/// (p50, p99) in µs over `iters` timed runs (after a short warmup).
fn timed(iters: usize, mut f: impl FnMut()) -> (u128, u128) {
    for _ in 0..3 {
        f();
    }
    let mut us: Vec<u128> = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = Instant::now();
        f();
        us.push(t.elapsed().as_micros());
    }
    us.sort_unstable();
    let at = |q: f64| us[(((us.len() - 1) as f64) * q) as usize];
    (at(0.5), at(0.99))
}

fn ms(u: u128) -> f64 {
    u as f64 / 1000.0
}

fn main() {
    // Amounts feasible even at the shallowest K=2 funnel (depths ~500–600).
    let req_in = request(e18(400), true);
    let req_out = request(e18(250), false);
    let per_leg = e18(1);

    println!("routing `solve_sparse` latency vs funnel size K — budget 500 ms p99");
    println!("finalised funnel is K ≤ 8 (research: production routers use 3–7 sources); larger K = context");
    println!("  K     exact-in (p50/p99)        exact-out (p50/p99)");
    let mut ok = true;
    // K ≤ 8 is the shipped range (gated); K ≥ 16 is shown for scaling context only.
    for k in [2u32, 4, 8, 16, 32] {
        let cs = funnel(k);
        let iters = if k <= 8 { 50 } else { 5 };
        let (i50, i99) = timed(iters, || {
            black_box(solve_sparse(black_box(&cs), &req_in, per_leg, 8, None));
        });
        let (o50, o99) = timed(iters, || {
            black_box(solve_sparse(black_box(&cs), &req_out, per_leg, 8, None));
        });
        let gated = k <= 8;
        let over = gated && (i99 > BUDGET_US || o99 > BUDGET_US);
        ok &= !over;
        println!(
            "  {k:<3}   p50={:>7.3} p99={:>7.3}ms   p50={:>7.3} p99={:>7.3}ms   {}",
            ms(i50),
            ms(i99),
            ms(o50),
            ms(o99),
            if gated {
                if over {
                    "OVER 500ms"
                } else {
                    "ok"
                }
            } else {
                "(context)"
            }
        );
        #[cfg(feature = "quote-metrics")]
        {
            reset_quote_calls();
            let _ = solve_sparse(black_box(&cs), &req_in, per_leg, 8, None);
            let in_calls = quote_calls();
            reset_quote_calls();
            let _ = solve_sparse(black_box(&cs), &req_out, per_leg, 8, None);
            let out_calls = quote_calls();
            println!("        quote-calls: exact-in {in_calls}, exact-out {out_calls}");
        }
        let _ = stdout().flush();
    }

    if !ok {
        eprintln!("routing latency budget (500 ms p99) EXCEEDED at a shipped K (<= 8)");
        std::process::exit(1);
    }
}
