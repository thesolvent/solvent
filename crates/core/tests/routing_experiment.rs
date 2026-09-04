//! Big-data routing study (test-only, never shipped). A catalog of permutations —
//! liquidity/trade tiers, curve-type mixes, stable vs volatile pairs — measuring how the
//! funnel ranking key and K affect output vs the all-pools optimum. Reproducible
//! (seeded), heterogeneous, and self-checking against a brute reference (`solve` over all
//! pools). Run a study: `cargo test -p solvent-core --release --lib routing::experiment::<name> -- --nocapture`.

use std::time::Instant;

use alloy_primitives::{Address, B256, U256};

use solvent_core::primitives::registry::{PeggedParams, StrategyKey};
use solvent_core::primitives::routing::RouteRequest;
use solvent_core::primitives::{IntentId, MakerId, StrategyHash};
use solvent_core::registry::{ConcentratePool, CurvePool, PeggedPool, XycPool};
use solvent_core::routing::solve;
use solvent_core::routing::Candidate;

/// Deterministic SplitMix64 — reproducible pools without a dependency.
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
fn as_u64(x: U256) -> u64 {
    u64::try_from(x).unwrap_or(u64::MAX)
}
fn curve_tag(c: &Candidate) -> char {
    match c.pool {
        CurvePool::Xyc(_) => 'X',
        CurvePool::Concentrate(_) => 'C',
        CurvePool::Pegged(_) => 'P',
        _ => '?',
    }
}

/// One family of pools + a trade to route through them.
#[allow(dead_code)] // `name`/`trade` document scenarios; not all studies read them
struct Scenario {
    name: &'static str,
    n: usize,
    depth: (u64, u64), // per-pool reserve in whole tokens
    skew: (u64, u64),  // price = skew/100 (reserve_out = depth·skew/100)
    mix: [u32; 3],     // relative weights for [Xyc, Concentrate, Pegged]
    stable_fees: bool, // stable pairs quote tighter, cheaper
    trade: U256,
}

fn gen_pool(rng: &mut Rng, id: u32, s: &Scenario) -> Candidate {
    let depth = e18(rng.range(s.depth.0, s.depth.1));
    let skew = rng.range(s.skew.0, s.skew.1);
    let reserve_in = depth;
    let reserve_out = depth * U256::from(skew) / U256::from(100u64);
    let fees = match s.stable_fees {
        true if rng.chance(50) => vec![1_000_000u32],
        true => vec![],
        false => match rng.range(0, 4) {
            0 => vec![],
            1 => vec![3_000_000u32],
            2 => vec![1_000_000u32],
            _ => vec![1_000_000u32, 2_000_000u32],
        },
    };
    let total = (s.mix[0] + s.mix[1] + s.mix[2]).max(1);
    let pick = rng.range(0, u64::from(total)) as u32;
    let pool = if pick < s.mix[0] {
        CurvePool::Xyc(XycPool::from_reserves(reserve_in, reserve_out))
    } else if pick < s.mix[0] + s.mix[1] {
        let sqrt_min = ((one18() / U256::from(4u64)) * one18()).root(2);
        let sqrt_max = (U256::from(4u64) * one18() * one18()).root(2);
        CurvePool::Concentrate(ConcentratePool::from_reserves_and_bounds(
            tok(1),
            tok(2),
            reserve_in,
            reserve_out,
            sqrt_min,
            sqrt_max,
        ))
    } else {
        CurvePool::Pegged(PeggedPool::from_reserves_and_params(
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
        ))
    };
    let cap = match rng.chance(20) {
        true => reserve_out * U256::from(rng.range(5, 50)) / U256::from(100u64),
        false => reserve_out * U256::from(10u64),
    };
    let mut hash = [0u8; 32];
    hash[28..].copy_from_slice(&id.to_be_bytes());
    let key = StrategyKey {
        maker: MakerId(Address::from([(id % 251 + 1) as u8; 20])),
        app: Address::ZERO,
        strategy_hash: StrategyHash(B256::from(hash)),
    };
    Candidate::new(key, tok(1), tok(2), cap, pool, fees)
}

fn pools(rng: &mut Rng, s: &Scenario) -> Vec<Candidate> {
    (0..s.n).map(|i| gen_pool(rng, i as u32, s)).collect()
}
fn request(trade: U256) -> RouteRequest {
    RouteRequest {
        intent: IntentId(B256::ZERO),
        token_in: tok(1),
        token_out: tok(2),
        amount: trade,
        exact_in: true,
    }
}

/// Candidate ranking keys — the funnel scores each pool, keeps the top K.
#[derive(Clone, Copy)]
enum Key {
    OutSize,      // estimated output for the whole trade (favors depth)
    OutFrac(u64), // output for trade/frac (blends depth and price)
    Spot,         // output for a tiny probe ≈ spot price (favors price)
    Cap,          // deliverable cap (the old T2 metric)
}
impl Key {
    fn name(self) -> String {
        match self {
            Key::OutSize => "out@size".into(),
            Key::OutFrac(f) => format!("out@1/{f}"),
            Key::Spot => "spot".into(),
            Key::Cap => "cap".into(),
        }
    }
    fn score(self, c: &Candidate, trade: U256) -> U256 {
        let at = |probe: U256| c.net_quote_exact_in(probe).unwrap_or(U256::ZERO);
        match self {
            Key::OutSize => at(trade),
            Key::OutFrac(f) => at((trade / U256::from(f)).max(U256::from(1u64))),
            Key::Spot => at((trade / U256::from(100_000u64)).max(U256::from(1u64))),
            Key::Cap => c.cap_out,
        }
    }
}

/// Top `k` candidates by `key` via quickselect (O(n)).
fn topk(cands: &[Candidate], trade: U256, key: Key, k: usize) -> Vec<Candidate> {
    let mut scored: Vec<(U256, usize)> = cands
        .iter()
        .enumerate()
        .map(|(i, c)| (key.score(c, trade), i))
        .collect();
    if scored.len() > k {
        scored.select_nth_unstable_by(k, |a, b| b.0.cmp(&a.0));
        scored.truncate(k);
    }
    scored.into_iter().map(|(_, i)| cands[i].clone()).collect()
}

fn regret_bps(reference: U256, got: U256) -> u64 {
    if reference.is_zero() {
        return 0;
    }
    as_u64(reference.saturating_sub(got) * U256::from(10_000u64) / reference)
}
fn tag_counts(cands: &[Candidate]) -> (usize, usize, usize) {
    cands
        .iter()
        .fold((0, 0, 0), |(x, c, p), cand| match curve_tag(cand) {
            'X' => (x + 1, c, p),
            'C' => (x, c + 1, p),
            _ => (x, c, p + 1),
        })
}

// ── Study A: liquidity × trade tiers ────────────────────────────────────────────

#[test]
#[ignore = "big-data study; run with --release --ignored"]
fn study_tiers() {
    const TRIALS: usize = 8;
    println!("\n=== STUDY A: liquidity × trade tiers (rank=out@size, mix=even) ===");
    println!("regret bps vs all-pools optimum at K=8/16/32, and avg active legs\n");
    println!("  liquidity      trade/depth   active    K=8    K=16   K=32");
    let liq = [
        ("small", (1u64, 10u64)),
        ("mid", (10, 1_000)),
        ("deep", (1_000, 100_000)),
    ];
    let ratios = [(1u64, "x0.1"), (10, "x1"), (100, "x10")];
    for (lname, drange) in liq {
        for (mult, rlabel) in ratios {
            let mid_depth = (drange.0 + drange.1) / 2;
            let trade = e18(mid_depth * mult / 10);
            let s = Scenario {
                name: lname,
                n: 100,
                depth: drange,
                skew: (60, 141),
                mix: [1, 1, 1],
                stable_fees: false,
                trade,
            };
            let (mut active, mut r8, mut r16, mut r32, mut ok) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for t in 0..TRIALS {
                let mut rng = Rng(0xA11CE ^ (t as u64).wrapping_mul(0x9E37));
                let cs = pools(&mut rng, &s);
                let Some(refr) = solve(&cs, &request(trade), None) else {
                    continue;
                };
                if refr.amount_out.is_zero() {
                    continue;
                }
                ok += 1;
                active += refr.legs.len() as u64;
                for (k, acc) in [(8, &mut r8), (16, &mut r16), (32, &mut r32)] {
                    let got = solve(&topk(&cs, trade, Key::OutSize, k), &request(trade), None)
                        .map(|x| x.amount_out)
                        .unwrap_or(U256::ZERO);
                    *acc += regret_bps(refr.amount_out, got);
                }
            }
            let ok = ok.max(1);
            println!(
                "  {lname:<6} {rlabel:>10}         {:>4}    {:>4}   {:>4}   {:>4}",
                active / ok,
                r8 / ok,
                r16 / ok,
                r32 / ok,
            );
        }
    }
    println!();
}

// ── Study B: curve-mix permutations + stable vs volatile ────────────────────────

#[test]
#[ignore = "big-data study; run with --release --ignored"]
fn study_curve_mix_and_stable() {
    const TRIALS: usize = 10;
    println!("\n=== STUDY B: curve mix + pair nature (rank=out@size, K=8) ===");
    println!("does ranking by output auto-prioritize the right curve? top-8 curve mix [X/C/P] + regret\n");
    let scenarios = [
        ("even mix, volatile ", [1, 1, 1], (60u64, 141u64), false),
        ("xyc-heavy, volatile", [2, 1, 1], (60, 141), false),
        ("pegged-heavy, volat", [1, 1, 2], (60, 141), false),
        ("STABLE pair (even)  ", [1, 1, 1], (98, 103), true),
        ("STABLE, xyc-heavy   ", [2, 1, 1], (98, 103), true),
    ];
    println!("  scenario              pool mix[X/C/P]   top8[X/C/P]   active[X/C/P]  regret");
    for (name, mix, skew, stable) in scenarios {
        let trade = e18(20_000); // large enough to spread
        let s = Scenario {
            name,
            n: 100,
            depth: (100, 20_000),
            skew,
            mix,
            stable_fees: stable,
            trade,
        };
        let (mut pt, mut tt, mut at, mut reg, mut ok) =
            ((0, 0, 0), (0, 0, 0), (0, 0, 0), 0u64, 0u64);
        for t in 0..TRIALS {
            let mut rng = Rng(0xB0B ^ (t as u64).wrapping_mul(0x1_2345));
            let cs = pools(&mut rng, &s);
            let Some(refr) = solve(&cs, &request(trade), None) else {
                continue;
            };
            if refr.amount_out.is_zero() {
                continue;
            }
            ok += 1;
            let top = topk(&cs, trade, Key::OutSize, 8);
            let pc = tag_counts(&cs);
            let tc = tag_counts(&top);
            let active: Vec<Candidate> = cs
                .iter()
                .filter(|c| {
                    refr.legs
                        .iter()
                        .any(|l| l.strategy_hash == c.key.strategy_hash)
                })
                .cloned()
                .collect();
            let ac = tag_counts(&active);
            pt = (pt.0 + pc.0, pt.1 + pc.1, pt.2 + pc.2);
            tt = (tt.0 + tc.0, tt.1 + tc.1, tt.2 + tc.2);
            at = (at.0 + ac.0, at.1 + ac.1, at.2 + ac.2);
            let got = solve(&top, &request(trade), None)
                .map(|x| x.amount_out)
                .unwrap_or(U256::ZERO);
            reg += regret_bps(refr.amount_out, got);
        }
        let d = ok.max(1) as usize;
        println!(
            "  {name}  {:>2}/{:>2}/{:>2}         {:>2}/{:>2}/{:>2}        {:>2}/{:>2}/{:>2}     {:>5}",
            pt.0 / d, pt.1 / d, pt.2 / d,
            tt.0 / d, tt.1 / d, tt.2 / d,
            at.0 / d, at.1 / d, at.2 / d,
            reg / ok.max(1),
        );
    }
    println!(
        "(P share rising in top8/active for the STABLE rows ⇒ output-ranking auto-picks pegged)\n"
    );
}

// ── Study C: ranking systems × K, across trade sizes ────────────────────────────

#[test]
#[ignore = "big-data study; run with --release --ignored"]
fn study_ranking_systems() {
    const TRIALS: usize = 8;
    println!("\n=== STUDY C: ranking systems × K (even mix, volatile) ===");
    println!("regret bps (lower=better) per ranking key\n");
    let keys = [Key::OutSize, Key::OutFrac(8), Key::Spot, Key::Cap];
    for (tlabel, trade) in [("small trade", e18(2_000)), ("large trade", e18(200_000))] {
        let s = Scenario {
            name: tlabel,
            n: 120,
            depth: (100, 50_000),
            skew: (60, 141),
            mix: [1, 1, 1],
            stable_fees: false,
            trade,
        };
        println!("  {tlabel} (avg active legs shown):");
        println!("    key         K=4    K=8    K=16   K=32");
        let mut active = 0u64;
        let mut acc = [[0u64; 4]; 4];
        let mut ok = 0u64;
        for t in 0..TRIALS {
            let mut rng = Rng(0xC0DE ^ (t as u64).wrapping_mul(0xABCD));
            let cs = pools(&mut rng, &s);
            let Some(refr) = solve(&cs, &request(trade), None) else {
                continue;
            };
            if refr.amount_out.is_zero() {
                continue;
            }
            ok += 1;
            active += refr.legs.len() as u64;
            for (kj, key) in keys.iter().enumerate() {
                for (ki, k) in [4usize, 8, 16, 32].iter().enumerate() {
                    let got = solve(&topk(&cs, trade, *key, *k), &request(trade), None)
                        .map(|x| x.amount_out)
                        .unwrap_or(U256::ZERO);
                    acc[kj][ki] += regret_bps(refr.amount_out, got);
                }
            }
        }
        let d = ok.max(1);
        println!("    (active legs avg: {})", active / d);
        for (kj, key) in keys.iter().enumerate() {
            println!(
                "    {:<10} {:>4}   {:>4}   {:>4}   {:>4}",
                key.name(),
                acc[kj][0] / d,
                acc[kj][1] / d,
                acc[kj][2] / d,
                acc[kj][3] / d,
            );
        }
        println!();
    }
}

// ── Uniform control: isolates capacity cost from ranking ────────────────────────

fn uniform_candidate(id: u32, depth: U256) -> Candidate {
    let mut hash = [0u8; 32];
    hash[28..].copy_from_slice(&id.to_be_bytes());
    let key = StrategyKey {
        maker: MakerId(Address::from([(id % 251 + 1) as u8; 20])),
        app: Address::ZERO,
        strategy_hash: StrategyHash(B256::from(hash)),
    };
    Candidate::new(
        key,
        tok(1),
        tok(2),
        depth * U256::from(10u64),
        CurvePool::Xyc(XycPool::from_reserves(depth, depth)),
        vec![],
    )
}

#[test]
#[ignore = "big-data study; run with --release --ignored"]
fn uniform_pools_isolate_capacity_cost() {
    const N: usize = 120;
    let trade = e18(2_000_000);
    println!("\n=== uniform control: {N} identical pools, ranking irrelevant ===");
    println!("regret = PURE capacity cost of using only K pools (no ranking effect)");
    for depth_k in [100_000u64, 500_000, 2_000_000] {
        let depth = e18(depth_k);
        let cands: Vec<Candidate> = (0..N as u32).map(|i| uniform_candidate(i, depth)).collect();
        let started = Instant::now();
        let refr = solve(&cands, &request(trade), None).expect("uniform reference");
        let ms = started.elapsed().as_secs_f64() * 1e3;
        print!(
            "  depth {:>4}k (trade/depth {:>4.1}): {} legs, ref {:.0}ms;  ",
            depth_k / 1000,
            2_000_000.0 / depth_k as f64,
            refr.legs.len(),
            ms,
        );
        for k in [2usize, 4, 8, 16, 32, 64] {
            let got = solve(&cands[..k.min(N)], &request(trade), None)
                .map(|s| s.amount_out)
                .unwrap_or(U256::ZERO);
            print!("K{k}={}bps ", regret_bps(refr.amount_out, got));
        }
        println!();
    }
    println!();
}
