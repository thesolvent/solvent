//! Tier-0 routing invariants (the property suite): properties every `solve` result must satisfy,
//! checked over a seeded random book generator — no reference solve, so each holds or reveals a
//! bug. Failures print the seed for a reproducible repro.
//!
//! The always-on tests (`cargo test -p solvent-core --test routing_properties`) are fast: the
//! sparsity check plus one pinned regression per fixed bug. The broad fuzz loops are `#[ignore]`d
//! because the debug BigRational solver is ~100 ms/solve; run them in release:
//! `cargo test -p solvent-core --test routing_properties --release -- --ignored`.

use alloy_primitives::{Address, B256, U256};

use solvent_core::primitives::registry::{PeggedParams, StrategyKey};
use solvent_core::primitives::routing::RouteRequest;
use solvent_core::primitives::{IntentId, MakerId, StrategyHash};
use solvent_core::registry::{ConcentratePool, CurvePool, PeggedPool, XycPool};
use solvent_core::routing::{solve, solve_sparse, Candidate, Split};

/// Deterministic SplitMix64 — reproducible books without a dependency.
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next_u64() % (hi - lo).max(1)
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.next_u64() % 100 < pct
    }
}

fn e18(n: u64) -> U256 {
    U256::from(n) * U256::from(10u64).pow(U256::from(18u64))
}
fn one27() -> U256 {
    U256::from(10u64).pow(U256::from(27u64))
}
fn tok(n: u8) -> Address {
    Address::from([n; 20])
}
fn umin(a: U256, b: U256) -> U256 {
    if a < b {
        a
    } else {
        b
    }
}

/// A random book of `n` candidates over `(tok(1), tok(2))`: varied curves, skews, fees, and caps,
/// with several strategies folded onto a smaller set of makers so shared wallets actually bind.
fn random_book(rng: &mut Rng, n: usize) -> Vec<Candidate> {
    let n_makers = rng.range(1, n as u64 + 1) as usize;
    // One shared wallet per maker (so a maker's strategies draw the same budget).
    let maker_wallet: Vec<U256> = (0..n_makers)
        .map(|_| e18(rng.range(100, 6000)) * U256::from(rng.range(2, 25)) / U256::from(10u64))
        .collect();
    (0..n)
        .map(|i| {
            let maker_idx = i % n_makers;
            let depth = e18(rng.range(100, 5000));
            let skew = rng.range(30, 300); // reserve_out = depth·skew/100
            let reserve_in = depth;
            let reserve_out = depth * U256::from(skew) / U256::from(100u64);
            let pool = match rng.range(0, 3) {
                0 => CurvePool::Xyc(XycPool::from_reserves(reserve_in, reserve_out)),
                1 => {
                    let sqrt_min = ((e18(1) / U256::from(4u64)) * e18(1)).root(2);
                    let sqrt_max = (U256::from(4u64) * e18(1) * e18(1)).root(2);
                    CurvePool::Concentrate(ConcentratePool::from_reserves_and_bounds(
                        tok(1),
                        tok(2),
                        reserve_in,
                        reserve_out,
                        sqrt_min,
                        sqrt_max,
                    ))
                }
                _ => CurvePool::Pegged(PeggedPool::from_reserves_and_params(
                    tok(1),
                    tok(2),
                    reserve_in,
                    reserve_out,
                    PeggedParams {
                        x0: reserve_in,
                        y0: reserve_out,
                        linear_width: U256::from(100u64) * one27(),
                        rate_lt: U256::from(1u64),
                        rate_gt: U256::from(1u64),
                    },
                )),
            };
            let fees = match rng.range(0, 4) {
                0 => vec![],
                1 => vec![3_000_000u32],
                2 => vec![1_000_000u32],
                _ => vec![1_000_000u32, 2_000_000u32],
            };
            let wallet = maker_wallet[maker_idx];
            let strategy_virtual = reserve_out * U256::from(rng.range(2, 30)) / U256::from(10u64);
            let cap_out = umin(wallet, strategy_virtual);
            let mut hash = [0u8; 32];
            hash[24..].copy_from_slice(&(i as u64).to_be_bytes());
            let key = StrategyKey {
                maker: MakerId(Address::from([(maker_idx as u8) + 1; 20])),
                app: Address::ZERO,
                strategy_hash: StrategyHash(B256::from(hash)),
            };
            Candidate::new(key, tok(1), tok(2), cap_out, wallet, pool, fees)
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

fn sum_of(xs: impl Iterator<Item = U256>) -> U256 {
    xs.fold(U256::ZERO, |s, x| s.saturating_add(x))
}

/// Every invariant a returned `Split` must satisfy. `where_` labels the failing case for a repro.
fn assert_invariants(cs: &[Candidate], req: &RouteRequest, sol: &Split, where_: &str) {
    // Totals equal the sum of legs.
    let sum_in = sum_of(sol.legs.iter().map(|l| l.amount_in));
    let sum_out = sum_of(sol.legs.iter().map(|l| l.amount_out));
    assert_eq!(
        sum_in, sol.amount_in,
        "{where_}: Σ leg.amount_in != amount_in"
    );
    assert_eq!(
        sum_out, sol.amount_out,
        "{where_}: Σ leg.amount_out != amount_out"
    );

    // No dust leg; token orientation matches the request.
    for l in &sol.legs {
        assert!(
            !l.amount_in.is_zero() && !l.amount_out.is_zero(),
            "{where_}: a zero-amount (dust) leg survived",
        );
        assert_eq!(l.token_in, req.token_in, "{where_}: leg token_in mismatch");
        assert_eq!(
            l.token_out, req.token_out,
            "{where_}: leg token_out mismatch"
        );
    }

    // Conservation vs the taker's target. Symmetric contract: exact-in spends at most the
    // target and within ε of it (a wallet-pinned book may strand a sub-ε sliver rather than
    // over-reserve); exact-out delivers at least the target and within ε of it.
    let eps = (req.amount / U256::from(1_000_000u64)).max(U256::from(1u64));
    if req.exact_in {
        assert!(
            sol.amount_in <= req.amount,
            "{where_}: exact-in over-spent the target ({} > {})",
            sol.amount_in,
            req.amount,
        );
        assert!(
            sol.amount_in >= req.amount.saturating_sub(eps),
            "{where_}: exact-in under-spent beyond ε ({} < {})",
            sol.amount_in,
            req.amount.saturating_sub(eps),
        );
    } else {
        assert!(
            sol.amount_out >= req.amount,
            "{where_}: exact-out under-delivered ({} < {})",
            sol.amount_out,
            req.amount,
        );
        assert!(
            sol.amount_out <= req.amount.saturating_add(eps),
            "{where_}: exact-out over-delivered beyond ε",
        );
    }

    // Per-leg output never exceeds its cap_out.
    for l in &sol.legs {
        let cap = cs
            .iter()
            .find(|c| c.key.strategy_hash == l.strategy_hash)
            .map(|c| c.cap_out)
            .expect("every leg maps to a candidate");
        assert!(
            l.amount_out <= cap,
            "{where_}: leg output {} exceeds its cap_out {}",
            l.amount_out,
            cap,
        );
    }

    // Each maker's combined output stays within its shared wallet.
    let mut makers: Vec<MakerId> = cs.iter().map(|c| c.key.maker).collect();
    makers.sort_unstable();
    makers.dedup();
    for maker in makers {
        let out = sum_of(
            sol.legs
                .iter()
                .filter(|l| l.maker == maker)
                .map(|l| l.amount_out),
        );
        let wallet = cs
            .iter()
            .find(|c| c.key.maker == maker)
            .map(|c| c.wallet_cap)
            .unwrap_or(U256::MAX);
        assert!(
            out <= wallet,
            "{where_}: maker's combined output {} exceeds its wallet {}",
            out,
            wallet,
        );
    }
}

/// One conservation trial: build the seeded book, solve, and assert every plan invariant.
fn conservation_trial(seed: u64) {
    let mut rng = Rng(0x5011_5011 ^ seed.wrapping_mul(0x9E37_79B9));
    let n = rng.range(1, 12) as usize;
    let cs = random_book(&mut rng, n);
    let amount = e18(rng.range(10, 4000));
    let exact_in = rng.chance(50);
    let req = request(amount, exact_in);
    let where_ = format!("seed={seed} n={n} amount={amount} exact_in={exact_in}");
    if let Some(sol) = solve(&cs, &req, None) {
        assert_invariants(&cs, &req, &sol, &where_);
    }
}

/// One order-independence trial: solve the book and its reversal, assert identical totals.
fn order_trial(seed: u64) {
    let mut rng = Rng(0xA11CE ^ seed.wrapping_mul(0x9E37_79B9));
    let n = rng.range(2, 12) as usize;
    let cs = random_book(&mut rng, n);
    let amount = e18(rng.range(10, 3000));
    let exact_in = rng.chance(50);
    let req = request(amount, exact_in);
    let mut reversed = cs.clone();
    reversed.reverse();
    match (solve(&cs, &req, None), solve(&reversed, &req, None)) {
        (Some(a), Some(b)) => assert_eq!(
            (a.amount_in, a.amount_out),
            (b.amount_in, b.amount_out),
            "seed={seed}: totals changed under candidate reordering",
        ),
        (None, None) => {}
        _ => panic!("seed={seed}: feasibility flipped under candidate reordering"),
    }
}

/// Exact-in stays within tolerance on a wallet-pinned book — the shape that first stranded a
/// sub-ε input sliver. Fast always-on guard against the under-spend regressing.
#[test]
fn exact_in_conservation_on_a_wallet_pinned_book() {
    conservation_trial(755);
}

/// Totals are identical under candidate reordering on a book with a marginal tie — the shape
/// that first exposed the order dependence. Fast always-on guard against it regressing.
#[test]
fn totals_are_order_independent_on_a_tie_book() {
    order_trial(183);
}

/// Broad conservation fuzz. Heavy under the debug BigRational solver (~100 ms/solve); run it in
/// release: `cargo test -p solvent-core --test routing_properties --release -- --ignored`.
#[test]
#[ignore = "heavy fuzz — run with --release --ignored"]
fn conservation_holds_over_random_books() {
    for seed in 0..1500 {
        conservation_trial(seed);
    }
}

/// Broad order-independence fuzz. Heavy in debug; run it in release (see the conservation fuzz).
#[test]
#[ignore = "heavy fuzz — run with --release --ignored"]
fn solve_is_order_independent_in_the_totals() {
    for seed in 0..400 {
        order_trial(seed);
    }
}

#[test]
fn sparsity_is_meaningful_below_the_gas_crossover() {
    // Eight equal, well-priced pools and a trade that fragments the gas-free optimum. With a
    // per-leg gas well below what each leg's output contributes, the sparsity pass must NOT
    // collapse to a single leg — each leg still earns its gas. (The existing test only exercises
    // the saturated branch where gas ≫ output.)
    let cs: Vec<Candidate> = (0u8..8)
        .map(|i| {
            let mut hash = [0u8; 32];
            hash[31] = i;
            let key = StrategyKey {
                maker: MakerId(Address::from([i + 1; 20])),
                app: Address::ZERO,
                strategy_hash: StrategyHash(B256::from(hash)),
            };
            Candidate::new(
                key,
                tok(1),
                tok(2),
                e18(10_000),
                U256::MAX,
                CurvePool::Xyc(XycPool::from_reserves(e18(2000), e18(2000))),
                vec![],
            )
        })
        .collect();
    let req = request(e18(3000), true);
    let free = solve(&cs, &req, None).unwrap();
    assert!(free.legs.len() >= 4, "gas-free optimum should fragment");

    // Gas ≈ 1% of a single leg's output — real but not overwhelming.
    let per_leg = e18(4);
    let sparse = solve_sparse(&cs, &req, per_leg, 8, None).unwrap();
    assert!(
        sparse.legs.len() > 1,
        "sub-crossover gas should keep multiple paying legs, not collapse (got {})",
        sparse.legs.len(),
    );
    assert_eq!(sparse.amount_in, e18(3000), "still spends the target");
}

/// A lower bound on the output a book can deliver, from per-candidate quotes and the shared
/// wallet caps alone — independent of the water-fill. Each leg delivers at most its `cap_out`
/// (when the curve can reach it) or its output at the large-input sentinel (its asymptote);
/// each maker's legs together deliver at most its wallet. Unpriceable legs count as zero, so
/// this never over-estimates — a target comfortably under it is genuinely fillable.
fn deliverable_lower_bound(cs: &[Candidate]) -> U256 {
    let sentinel = U256::from(1u8) << 112;
    let leg_out = |c: &Candidate| match c.input_within_output(c.cap_out) {
        Some(_) => c.cap_out,
        None => c
            .net_quote_exact_in(sentinel)
            .unwrap_or(U256::ZERO)
            .min(c.cap_out),
    };
    let mut makers: Vec<MakerId> = cs.iter().map(|c| c.key.maker).collect();
    makers.sort_unstable();
    makers.dedup();
    makers.iter().fold(U256::ZERO, |total, &m| {
        let group = cs
            .iter()
            .filter(|c| c.key.maker == m)
            .fold(U256::ZERO, |s, c| s.saturating_add(leg_out(c)));
        let wallet = cs
            .iter()
            .find(|c| c.key.maker == m)
            .map(|c| c.wallet_cap)
            .unwrap_or(U256::MAX);
        total.saturating_add(group.min(wallet))
    })
}

/// One feasibility trial: a book that can comfortably deliver a target must not decline it. The
/// target is four-fifths of a conservative capacity bound — well within reach.
fn feasibility_trial(seed: u64) {
    let mut rng = Rng(0xFEA5_1B1E ^ seed.wrapping_mul(0x9E37_79B9));
    let n = rng.range(1, 12) as usize;
    let cs = random_book(&mut rng, n);
    let target = deliverable_lower_bound(&cs) * U256::from(4u64) / U256::from(5u64);
    if target.is_zero() {
        return;
    }
    assert!(
        solve(&cs, &request(target, false), None).is_some(),
        "seed={seed}: declined an exact-out target {target} at 80% of a capacity lower bound",
    );
}

/// A comfortably-fillable exact-out target near a leg's capacity is not declined — the book that
/// first exposed the feasibility-gate under-count (`measure(0)` capped a wei below target). Fast
/// always-on guard.
#[test]
fn exact_out_near_capacity_is_not_declined() {
    feasibility_trial(13);
}

/// Feasibility oracle: catches a spurious `None` on a fillable book — a single leg erroring
/// inside `build`, or the feasibility gate under-counting. This is the gap the conservation
/// suite can't see, since it only checks the plans that come back `Some`.
#[test]
#[ignore = "heavy fuzz — run with --release --ignored"]
fn feasible_books_are_not_declined() {
    for seed in 0u64..1500 {
        feasibility_trial(seed);
    }
}
