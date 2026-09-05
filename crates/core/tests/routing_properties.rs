//! Tier-0 routing invariants (the property suite): properties every `solve` result must satisfy,
//! checked over a seeded random book generator — no reference solve, so each holds or reveals a
//! bug. Failures print the seed for a reproducible repro.
//!
//! The always-on tests (`cargo test -p solvent-core --test routing_properties`) are fast: the
//! sparsity check plus one pinned regression per fixed bug. The broad fuzz loops are `#[ignore]`d
//! because the debug BigRational solver is ~100 ms/solve; run them in release:
//! `cargo test -p solvent-core --test routing_properties --release -- --ignored`.

use alloy_primitives::{Address, B256, U256};

use solvent_core::primitives::pricing::Ratio;
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

/// A leg's finite-difference net marginal `d(output)/d(gross_input)` over `[fill, fill + probe]`.
/// A ppm-scale `probe` tracks the tangent at `fill`; a coarser one gives the average marginal a
/// would-be fill of that size would earn. `None` if the leg can't price the step.
fn fd_marginal(c: &Candidate, fill: U256, probe: U256) -> Option<Ratio> {
    let probe = probe.max(U256::from(1u64));
    let here = c.net_quote_exact_in(fill).ok()?;
    let ahead = c.net_quote_exact_in(fill.saturating_add(probe)).ok()?;
    Ratio::new(ahead.saturating_sub(here), probe)
}

/// The KKT optimality residuals of a solved split, stated intrinsically (no dependence on the
/// reported λ, which degenerates to zero on near-capacity trades). `residual_bps` is the spread
/// of active interior legs' marginals — zero iff they share one marginal (the equimarginal
/// condition); paired with conservation, that pins the correct water level. `violation_bps` is
/// how far an unused leg's spot marginal rises above the cheapest interior leg — positive means
/// it should have been filled. Box-capped and wallet-bound legs are excluded (complementary
/// slackness on a cap, not the equimarginal condition).
struct Certificate {
    residual_bps: u64,
    violation_bps: u64,
}

fn kkt_certificate(cs: &[Candidate], req: &RouteRequest, split: &Split) -> Certificate {
    // An unused leg is probed over a chunk of the trade — whether a would-be fill of that size
    // would still beat the water level (spot alone over-counts steep-decay legs whose optimal
    // fill is nil). An active leg's marginal is probed locally, a small fraction of its own fill,
    // so a steep leg with a small fill isn't read low by a trade-scaled step overshooting it.
    let chunk = (req.amount / U256::from(16u64)).max(U256::from(1u64));
    let group_tol = (req.amount / U256::from(1_000_000u64)).max(U256::from(1u64));
    let group_out = |maker: MakerId| {
        split
            .legs
            .iter()
            .filter(|l| l.maker == maker)
            .fold(U256::ZERO, |s, l| s.saturating_add(l.amount_out))
    };
    let mut interior: Vec<Ratio> = Vec::new();
    let mut unused: Vec<Ratio> = Vec::new();
    for c in cs {
        let leg = split
            .legs
            .iter()
            .find(|l| l.strategy_hash == c.key.strategy_hash);
        let fill_in = leg.map_or(U256::ZERO, |l| l.amount_in);
        let fill_out = leg.map_or(U256::ZERO, |l| l.amount_out);
        // Cap-relative slack, so a leg the conservative bound parks a few ppm below its cap still
        // reads as box-capped.
        let cap_tol = (c.cap_out / U256::from(100_000u64)).max(U256::from(1u64));
        // Wallet-bound group: pinned at the wallet, not at the equimarginal level.
        if group_out(c.key.maker).saturating_add(group_tol) >= c.wallet_cap {
            continue;
        }
        // Excluded from the equimarginal residual: a sub-ppm fill (activation-margin rounding), a
        // box-capped leg (pinned by its own cap), or a pegged leg (its numerical fill lands the
        // marginal on λ only to a finite-difference tolerance, not exactly). All three are still
        // covered by the unused-leg violation check.
        let is_pegged = matches!(c.pool, CurvePool::Pegged(_));
        let excluded =
            fill_in < group_tol || fill_out.saturating_add(cap_tol) >= c.cap_out || is_pegged;
        match (fill_in.is_zero(), excluded) {
            // Only a leg whose maker solve used *nothing* of is a candidate for "should have been
            // filled" — an unused leg of a participating maker is its shared wallet spent on a
            // better pool, not a miss.
            (true, _) if group_out(c.key.maker).is_zero() => {
                unused.extend(fd_marginal(c, U256::ZERO, chunk))
            }
            (true, _) => {}
            (false, true) => {}
            (false, false) => {
                let local = (fill_in / U256::from(1000u64)).max(U256::from(1u64));
                interior.extend(fd_marginal(c, fill_in, local));
            }
        }
    }
    let cheapest = interior.iter().min();
    let residual_bps = match (cheapest, interior.iter().max()) {
        (Some(lo), Some(hi)) => lo.rel_diff_bps(hi),
        _ => 0,
    };
    let violation_bps = cheapest.map_or(0, |lo| {
        unused
            .iter()
            .filter(|m| *m > lo)
            .map(|m| m.rel_diff_bps(lo))
            .max()
            .unwrap_or(0)
    });
    Certificate {
        residual_bps,
        violation_bps,
    }
}

/// One KKT trial on an interior exact-in solve, returning its certificate (`None` when the book
/// yields no interior trade to check). An interior input is found by solving a comfortably
/// sub-capacity exact-out trade and re-sourcing its input exact-in — exact-in has no overshoot
/// trim, so its marginals sit cleanly at the water level.
fn kkt_trial(seed: u64) -> Option<Certificate> {
    let mut rng = Rng(0xC0FFEE ^ seed.wrapping_mul(0x9E37_79B9));
    let n = rng.range(1, 12) as usize;
    let cs = random_book(&mut rng, n);
    let out_target = deliverable_lower_bound(&cs) / U256::from(3u64);
    if out_target.is_zero() {
        return None;
    }
    let in_target = solve(&cs, &request(out_target, false), None)?.amount_in;
    if in_target.is_zero() {
        return None;
    }
    let req = request(in_target, true);
    let sol = solve(&cs, &req, None)?;
    Some(kkt_certificate(&cs, &req, &sol))
}

/// A comfortably interior exact-in solve sits at the equimarginal optimum — the book that first
/// exposed the pegged pool being dropped at a positive water level. Fast always-on guard.
#[test]
fn solve_is_kkt_optimal_on_a_pegged_book() {
    if let Some(cert) = kkt_trial(386) {
        assert!(
            cert.residual_bps <= 25 && cert.violation_bps <= 25,
            "seed=386: KKT residual={} violation={} (bps) — not at the equimarginal optimum",
            cert.residual_bps,
            cert.violation_bps,
        );
    }
}

/// Optimality oracle: every interior exact-in solve sits at the equimarginal (KKT) optimum —
/// active legs share one marginal and no unused leg beats them. Catches a suboptimal split (wrong
/// λ or wrong leg selection) that the validity and feasibility oracles can't see.
#[test]
#[ignore = "heavy fuzz — run with --release --ignored"]
fn solve_is_kkt_optimal() {
    for seed in 0u64..1500 {
        if let Some(cert) = kkt_trial(seed) {
            assert!(
                cert.residual_bps <= 25,
                "seed={seed}: active legs off the equimarginal level by {} bps",
                cert.residual_bps,
            );
            assert!(
                cert.violation_bps <= 25,
                "seed={seed}: an unused leg beats the split by {} bps",
                cert.violation_bps,
            );
        }
    }
}

/// Round-trip: sourcing `X` input exact-in yields `Y` output, and buying `Y` output exact-out
/// should cost `~X` again. Returns `|X − X'|` in bps of `X` (`None` when the book has no interior
/// trade). Catches asymmetry between the two assemble paths.
fn roundtrip_bps(seed: u64) -> Option<u64> {
    let mut rng = Rng(0x0022_11AA ^ seed.wrapping_mul(0x9E37_79B9));
    let n = rng.range(1, 12) as usize;
    let cs = random_book(&mut rng, n);
    let out_target = deliverable_lower_bound(&cs) / U256::from(3u64);
    if out_target.is_zero() {
        return None;
    }
    let x = solve(&cs, &request(out_target, false), None)?.amount_in;
    let y = solve(&cs, &request(x, true), None)?.amount_out;
    if x.is_zero() || y.is_zero() {
        return None;
    }
    let x2 = solve(&cs, &request(y, false), None)?.amount_in;
    Some(Ratio::from(x2).rel_diff_bps(&Ratio::from(x)))
}

/// Round-trip consistency holds on a book: exact-in X → Y, then exact-out Y → X' returns to X
/// within a bp. Fast always-on guard.
#[test]
fn roundtrip_is_consistent_on_a_book() {
    if let Some(bps) = roundtrip_bps(7) {
        assert!(bps <= 20, "round-trip off by {bps} bps");
    }
}

/// Round-trip oracle: the two assemble paths agree — sourcing X input exact-in then buying that
/// output exact-out returns to ~X. Catches asymmetry between the top-up and the trim.
#[test]
#[ignore = "heavy fuzz — run with --release --ignored"]
fn solve_round_trips_between_directions() {
    for seed in 0u64..1500 {
        if let Some(bps) = roundtrip_bps(seed) {
            assert!(bps <= 20, "seed={seed}: round-trip off by {bps} bps");
        }
    }
}

/// Output-monotonicity breaks on a fixed book swept over increasing exact-in sizes: more input
/// must never buy less output. Returns the count of breaks (0 = clean). (λ-monotonicity is not
/// asserted — the water level is only resolved to the bisection tolerance, so it can tick the
/// wrong way by a hair; a non-monotone `measure(λ)` would instead surface as a conservation or
/// KKT failure, which it does not.)
fn output_monotonicity_breaks(seed: u64) -> u32 {
    let mut rng = Rng(0x30D0_30D0 ^ seed.wrapping_mul(0x9E37_79B9));
    let n = rng.range(1, 12) as usize;
    let cs = random_book(&mut rng, n);
    let out_cap = deliverable_lower_bound(&cs);
    if out_cap.is_zero() {
        return 0;
    }
    let Some(x_cap) = solve(&cs, &request(out_cap / U256::from(2u64), false), None) else {
        return 0;
    };
    let x_cap = x_cap.amount_in;
    let mut breaks = 0;
    let mut prev_out = U256::ZERO;
    for k in 1u64..=8 {
        let x = x_cap * U256::from(k) / U256::from(8u64);
        let Some(sol) = solve(&cs, &request(x, true), None) else {
            continue;
        };
        if sol.amount_out < prev_out {
            breaks += 1;
        }
        prev_out = sol.amount_out;
    }
    breaks
}

/// Output rises with trade size on a book — more input never buys less output. Fast always-on
/// guard.
#[test]
fn output_rises_with_trade_size_on_a_book() {
    assert_eq!(
        output_monotonicity_breaks(7),
        0,
        "output decreased as input grew"
    );
}

/// Monotonicity oracle: solve output is non-decreasing in trade size across the random books — a
/// dip would be an arbitrageable quote.
#[test]
#[ignore = "heavy fuzz — run with --release --ignored"]
fn solve_output_is_monotone_in_trade_size() {
    // Fewer books than the other fuzzes: this one solves a full trade-size sweep per book.
    for seed in 0u64..500 {
        assert_eq!(
            output_monotonicity_breaks(seed),
            0,
            "seed={seed}: output decreased as input grew",
        );
    }
}

/// The best gas-aware `net_output` any non-empty subset of a (small) book achieves at
/// `per_leg_cost` — the true optimum `solve_sparse` is approximating.
fn brute_sparse_optimum(cs: &[Candidate], req: &RouteRequest, per_leg_cost: U256) -> U256 {
    let mut best = U256::ZERO;
    for mask in 1u32..(1u32 << cs.len()) {
        let subset: Vec<Candidate> = cs
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, c)| c.clone())
            .collect();
        if let Some(split) = solve(&subset, req, None) {
            best = best.max(split.net_output(per_leg_cost));
        }
    }
    best
}

/// `solve_sparse`'s regret against the true `2^K` gas-aware optimum, in bps of the optimum's net
/// output. `gas_div` sets the per-leg gas as `avg_leg_output / gas_div` (so `gas_div=50` ≈ 2 %
/// of a leg's output, a realistic swap cost). `None` when there's no multi-leg trade to prune.
fn sparsity_regret_bps(seed: u64, gas_div: u64) -> Option<u64> {
    let mut rng = Rng(0x5A17_5A17 ^ seed.wrapping_mul(0x9E37_79B9));
    let n = rng.range(2, 7) as usize;
    let cs = random_book(&mut rng, n);
    let out_cap = deliverable_lower_bound(&cs);
    if out_cap.is_zero() {
        return None;
    }
    let x = solve(&cs, &request(out_cap / U256::from(3u64), false), None)?.amount_in;
    if x.is_zero() {
        return None;
    }
    let req = request(x, true);
    let free = solve(&cs, &req, None)?;
    if free.legs.len() < 2 {
        return None;
    }
    let per_leg = free.amount_out / U256::from(gas_div * free.legs.len() as u64);
    if per_leg.is_zero() {
        return None;
    }
    let ours = solve_sparse(&cs, &req, per_leg, n, None)?.net_output(per_leg);
    let best = brute_sparse_optimum(&cs, &req, per_leg);
    if best.is_zero() {
        return None;
    }
    Some(Ratio::from(ours).rel_diff_bps(&Ratio::from(best)))
}

/// `gas_div = 50` ⇒ per-leg gas ≈ 2 % of a leg's output, a realistic swap cost. At that regime
/// the drop-by-smallest heuristic is near-optimal (observed regret ≤ 37 bps); regret grows only
/// as gas approaches a large fraction of a leg's output (uneconomical trades), logged as L8.
const REALISTIC_GAS_DIV: u64 = 50;

/// `solve_sparse` stays near the true optimum on a book at realistic gas — the worst realistic-gas
/// case in the sweep. Fast always-on guard.
#[test]
fn sparse_is_near_optimal_at_realistic_gas() {
    if let Some(bps) = sparsity_regret_bps(59, REALISTIC_GAS_DIV) {
        assert!(bps <= 100, "sparsity regret {bps} bps at realistic gas");
    }
}

/// Sparsity-optimality oracle: at realistic gas, `solve_sparse`'s drop-the-smallest heuristic is
/// within a small regret of the true `2^K` gas-aware optimum (enumerated over all subsets). K ≤ 6
/// keeps the enumeration tractable.
#[test]
#[ignore = "heavy fuzz — run with --release --ignored"]
fn solve_sparse_is_near_optimal() {
    for seed in 0u64..300 {
        if let Some(bps) = sparsity_regret_bps(seed, REALISTIC_GAS_DIV) {
            assert!(
                bps <= 100,
                "seed={seed}: sparsity regret {bps} bps vs the true optimum at realistic gas",
            );
        }
    }
}

/// Funnel regret at `k`, decomposed. `capacity_bps` = output lost even when the funnel keeps the
/// `k` pools the all-pools optimum relied on most — the intrinsic cost of a small funnel, in bps
/// of the all-pools output. `ranking_bps` = the *extra* loss from ranking by output-at-size instead
/// of that oracle support (pure ranking error), in bps of the oracle's output.
struct FunnelRegret {
    capacity_bps: u64,
    /// `None` when the oracle's K-subset can't even fill the trade (K is capacity-limited, so
    /// ranking is undefined); `Some(bps)` when it can, measuring out@size's extra loss.
    ranking_bps: Option<u64>,
}

fn funnel_regret(seed: u64, k: usize) -> Option<FunnelRegret> {
    let mut rng = Rng(0x00F0_DD1E ^ seed.wrapping_mul(0x9E37_79B9));
    let n = rng.range(k as u64 + 1, 12) as usize;
    let cs = random_book(&mut rng, n);
    let out_cap = deliverable_lower_bound(&cs);
    if out_cap.is_zero() {
        return None;
    }
    let x = solve(&cs, &request(out_cap / U256::from(3u64), false), None)?.amount_in;
    if x.is_zero() {
        return None;
    }
    let req = request(x, true);
    let all = solve(&cs, &req, None)?;
    if all.amount_out.is_zero() {
        return None;
    }
    let solve_out =
        |subset: &[Candidate]| solve(subset, &req, None).map_or(U256::ZERO, |s| s.amount_out);
    let top_k = |ranked: Vec<&Candidate>| -> U256 {
        let subset: Vec<Candidate> = ranked.into_iter().take(k).cloned().collect();
        solve_out(&subset)
    };
    // out@size ranking (what `select` uses): net output at the trade size, ties by strategy_hash.
    let mut by_size: Vec<&Candidate> = cs.iter().collect();
    by_size.sort_by(|a, b| {
        let (sa, sb) = (
            a.net_quote_exact_in(x).unwrap_or(U256::ZERO),
            b.net_quote_exact_in(x).unwrap_or(U256::ZERO),
        );
        sb.cmp(&sa)
            .then(a.key.strategy_hash.cmp(&b.key.strategy_hash))
    });
    let o_size = top_k(by_size);
    // Oracle ranking: the pools the all-pools optimum put the most output into.
    let mut by_fill: Vec<&Candidate> = cs.iter().collect();
    let fill_of = |c: &Candidate| {
        all.legs
            .iter()
            .find(|l| l.strategy_hash == c.key.strategy_hash)
            .map_or(U256::ZERO, |l| l.amount_out)
    };
    by_fill.sort_by(|a, b| {
        fill_of(b)
            .cmp(&fill_of(a))
            .then(a.key.strategy_hash.cmp(&b.key.strategy_hash))
    });
    let o_oracle = top_k(by_fill);
    Some(FunnelRegret {
        capacity_bps: Ratio::from(o_oracle).rel_diff_bps(&Ratio::from(all.amount_out)),
        ranking_bps: match () {
            _ if o_oracle.is_zero() => None,
            _ if o_size >= o_oracle => Some(0),
            _ => Some(Ratio::from(o_size).rel_diff_bps(&Ratio::from(o_oracle))),
        },
    })
}

/// Funnel decomposition study: at a forced-small K, how much of the top-K funnel's loss is the
/// intrinsic capacity cost of keeping only K pools, versus out@size ranking picking a worse K than
/// the all-pools optimum's support. Prints the distribution; the finding (out@size is capacity-
/// blind) is L9. Not an assertion — it characterizes a known deficiency, not a pass/fail invariant.
#[test]
#[ignore = "study — run with --release --ignored --nocapture"]
fn study_funnel_decomposition() {
    for k in [2usize, 4] {
        let (mut mcap, mut mrank, mut checked, mut cap_limited, mut rank_over50) =
            (0u64, 0u64, 0u32, 0u32, 0u32);
        let (mut wc, mut wr) = (String::new(), String::new());
        for seed in 0u64..600 {
            if let Some(fr) = funnel_regret(seed, k) {
                checked += 1;
                if fr.capacity_bps > mcap {
                    mcap = fr.capacity_bps;
                    wc = format!("seed={seed}");
                }
                match fr.ranking_bps {
                    None => cap_limited += 1,
                    Some(r) => {
                        if r > 50 {
                            rank_over50 += 1;
                        }
                        if r > mrank {
                            mrank = r;
                            wr = format!("seed={seed}");
                        }
                    }
                }
            }
        }
        println!("K={k} over {checked}: capacity_max {mcap} [{wc}] cap_limited {cap_limited}  ranking_max {mrank} [{wr}] ranking_over50 {rank_over50}");
    }
}
