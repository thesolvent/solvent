//! Candidate selection: reduce an intent's pair to a bounded, deterministic set of
//! priceable maker venues for the solver to optimize over.
//!
//! Ranks every eligible venue by its **estimated net-of-fee output at the trade size** —
//! the metric production routers select on, which self-selects the right curve type — and
//! keeps the top-`k`. Because our quotes are closed-form, we rank directly rather than with
//! a cheap TVL prefilter. Caps are frozen by reading the ledger snapshot once; the ledger
//! re-checks the firm figure at reservation time.

use std::cmp::Ordering;

use alloy_primitives::{Address, U256};

use crate::ledger::AvailableSnapshot;
use crate::primitives::ledger::AccountKey;
use crate::primitives::pricing::{LimitedQuote, Ratio};
use crate::primitives::registry::{CurveSpec, MakerStrategy, Snapshot, StrategyKey, TokenPair};
use crate::primitives::routing::RouteRequest;
use crate::registry::{gross_up_by_fees, shrink_by_fees, CurveError, CurvePool, Pricing};

/// A frozen, priceable maker venue for one `token_in -> token_out` direction. Carries
/// the oriented curve and its flat fees so the solver can price it net-of-fee.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Candidate {
    pub key: StrategyKey,
    pub token_in: Address,
    pub token_out: Address,
    /// Max output this single leg can deliver — the frozen `min(wallet budget, strategy
    /// virtual)`.
    pub cap_out: U256,
    /// The maker's `token_out` wallet, shared across its strategies — the cap on their
    /// combined output.
    pub wallet_cap: U256,
    pub pool: CurvePool,
    pub fees_in_bps: Vec<u32>,
}

/// Aqua flat-fee denominator (`Fee.BPS`, 1e9 = 100%).
const BPS: u64 = 1_000_000_000;

/// Per-thread count of curve-quote requests, for the latency benchmark's cost model. Compiled
/// only under `quote-metrics`, so production carries no counter.
#[cfg(feature = "quote-metrics")]
mod metrics {
    use std::cell::Cell;
    thread_local! { static CALLS: Cell<u64> = const { Cell::new(0) }; }
    pub(super) fn bump() {
        CALLS.with(|c| c.set(c.get() + 1));
    }
    /// Quote requests counted since the last reset.
    pub fn quote_calls() -> u64 {
        CALLS.with(Cell::get)
    }
    /// Zero the counter.
    pub fn reset_quote_calls() {
        CALLS.with(|c| c.set(0));
    }
}
#[cfg(feature = "quote-metrics")]
pub use metrics::{quote_calls, reset_quote_calls};

impl Candidate {
    /// A frozen venue; built internally by [`select`], public so tests and tools can build one.
    pub fn new(
        key: StrategyKey,
        token_in: Address,
        token_out: Address,
        cap_out: U256,
        wallet_cap: U256,
        pool: CurvePool,
        fees_in_bps: Vec<u32>,
    ) -> Self {
        Self {
            key,
            token_in,
            token_out,
            cap_out,
            wallet_cap,
            pool,
            fees_in_bps,
        }
    }

    /// γ = Π(1 − fⱼ/BPS): the fraction of gross input the fee-free curve actually sees.
    fn gamma(&self) -> Result<Ratio, CurveError> {
        self.fees_in_bps
            .iter()
            .try_fold(Ratio::from(U256::from(1u64)), |g, &f| {
                let factor = Ratio::new(U256::from(BPS - u64::from(f)), U256::from(BPS))
                    .ok_or(CurveError::DivByZero)?;
                Ok(g * factor)
            })
    }

    /// Fee-inclusive output for a gross input: flat fees shrink the input, then the curve.
    pub fn net_quote_exact_in(&self, gross_in: U256) -> Result<U256, CurveError> {
        #[cfg(feature = "quote-metrics")]
        metrics::bump();
        self.pool
            .quote_exact_in(shrink_by_fees(gross_in, &self.fees_in_bps)?)
    }

    /// Fee-inclusive gross input for an output: the curve, then fees gross the input up.
    pub fn net_quote_exact_out(&self, amount_out: U256) -> Result<U256, CurveError> {
        #[cfg(feature = "quote-metrics")]
        metrics::bump();
        gross_up_by_fees(self.pool.quote_exact_out(amount_out)?, &self.fees_in_bps)
    }

    /// The largest gross input whose fee-inclusive output does not exceed `max_out`. Unlike
    /// [`net_quote_exact_out`](Self::net_quote_exact_out), which rounds the input up to deliver
    /// *at least* `max_out`, this steps back a unit when that round-trip overshoots — so a leg
    /// filled to it never exceeds its cap. `None` if the pool can't price `max_out`.
    pub fn input_within_output(&self, max_out: U256) -> Option<U256> {
        let input = self.net_quote_exact_out(max_out).ok()?;
        match self.net_quote_exact_in(input) {
            Ok(out) if out <= max_out => Some(input),
            Ok(_) => Some(input.saturating_sub(U256::from(1u64))),
            Err(_) => None,
        }
    }

    /// Fee-inclusive fill up to a gross marginal `limit`: rescale the bound to net space
    /// (net marginal = gross/γ, so use λ/γ), fill the fee-free pool, then gross the
    /// consumed input back up. The split decision; leg amounts are set exactly later.
    pub fn net_quote_with_limit(
        &self,
        gross_bound: U256,
        limit: &Ratio,
    ) -> Result<LimitedQuote, CurveError> {
        #[cfg(feature = "quote-metrics")]
        metrics::bump();
        let net_limit = limit.clone() * self.gamma()?.invert().ok_or(CurveError::DivByZero)?;
        let net_bound = shrink_by_fees(gross_bound, &self.fees_in_bps)?;
        let net = self.pool.quote_with_limit(net_bound, &net_limit)?;
        Ok(LimitedQuote {
            amount_in: gross_up_by_fees(net.amount_in, &self.fees_in_bps)?.min(gross_bound),
            amount_out: net.amount_out,
            limited: net.limited,
        })
    }

    /// Probe output below which integer truncation dominates the rate it implies.
    const MIN_PROBE_OUT: u64 = 1_000_000;

    /// Fractions of the trade size to probe the marginal rate at, narrowest first.
    const PROBE_DIVISORS: [u64; 3] = [1_000_000, 1_000, 1];

    /// A conservative estimate of the pool's spot marginal (net output-per-input as the trade
    /// → 0): the secant slope over a tiny probe. For a concave curve the secant lies below the
    /// true tangent, so this under-estimates — the certificate built on it may miss a case,
    /// never false-alarm. A diagnostic, not a proof.
    ///
    /// The probe must also be big enough to survive the quote truncating to whole base units: one
    /// resolving to a handful of them implies a rate wrong by tens of percent, which then reads as
    /// price impact. So widen it until its output can be divided meaningfully, and failing that
    /// take the trade size itself, where impact is genuinely below what can be measured.
    pub fn spot_marginal(&self, amount: U256) -> Option<Ratio> {
        let mut marginal = None;
        for divisor in Self::PROBE_DIVISORS {
            let probe = (amount / U256::from(divisor)).max(U256::from(1u64));
            let Ok(out) = self.net_quote_exact_in(probe) else {
                break;
            };
            marginal = Ratio::new(out, probe);
            if out >= U256::from(Self::MIN_PROBE_OUT) {
                break;
            }
        }
        marginal
    }
}

/// The most promising venues for a request, deterministically ordered and capped at `k`.
/// Ranks by **estimated net-of-fee output at the trade size** (exact-out: the input to
/// deliver it) — the metric every production router selects on, and the one that makes the
/// solver's optimum captured by a small `k`. Since our quotes are closed-form we rank every
/// candidate directly (no cheap TVL prefilter), which is strictly more accurate. Top-`k` via
/// quickselect (`O(n)`); `strategy_hash` breaks ties for a snapshot-order-independent set.
pub fn select(
    snapshot: &Snapshot,
    caps: &AvailableSnapshot,
    request: &RouteRequest,
    k: usize,
) -> Selection {
    let pair = TokenPair::new(request.token_in, request.token_out);
    // What the taker cares about, quoted once per candidate: output for exact-in, required
    // input for exact-out. `None` = can't price this size.
    let score = |c: &Candidate| -> Option<U256> {
        match request.exact_in {
            true => c.net_quote_exact_in(request.amount).ok(),
            false => c.net_quote_exact_out(request.amount.min(c.cap_out)).ok(),
        }
    };
    let mut scored: Vec<Scored> = snapshot
        .active_strategies_for_pair(pair)
        .filter_map(|s| build_candidate(s, caps, request.token_in, request.token_out))
        .map(|candidate| Scored {
            score: score(&candidate),
            candidate,
        })
        .collect();
    let exact_in = request.exact_in;
    let better = |a: &Scored, b: &Scored| best_first(a, b, exact_in);
    // Partition off the top-`k`; the discarded tail feeds the certificate diagnostic — the
    // highest (conservatively-estimated) spot marginal an omitted pool could have offered.
    let best_omitted_spot = if scored.len() > k {
        scored.select_nth_unstable_by(k, better);
        let spot = scored[k..]
            .iter()
            .filter_map(|s| s.candidate.spot_marginal(request.amount))
            .max();
        scored.truncate(k);
        spot
    } else {
        None
    };
    scored.sort_by(better);
    Selection {
        chosen: scored.into_iter().map(|s| s.candidate).collect(),
        best_omitted_spot,
    }
}

/// The funnel's output: the top-`k` candidates, and the highest spot marginal among the pools
/// that didn't make the cut (`None` if none were dropped). Feeds a **heuristic** certificate:
/// out@size ranking isn't a proven superset of the optimal support, so this flags a probably-
/// too-small `k` — a diagnostic, not a guarantee.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Selection {
    pub chosen: Vec<Candidate>,
    pub best_omitted_spot: Option<Ratio>,
}

/// The blended rate's relative shortfall from the best (near-zero-impact) rate, in percent. The
/// best rate is the tightest candidate's spot marginal, read straight off the routed candidates —
/// no second solve.
pub fn price_impact_pct(chosen: &[Candidate], amount_in: U256, amount_out: U256) -> f64 {
    let best = chosen
        .iter()
        .filter_map(|c| c.spot_marginal(amount_in))
        .max();
    match (best, Ratio::new(amount_out, amount_in)) {
        (Some(best), Some(effective)) => effective.rel_diff_bps(&best) as f64 / 100.0,
        _ => 0.0,
    }
}

/// A candidate with its rank score (`None` = unpriceable at this size).
struct Scored {
    score: Option<U256>,
    candidate: Candidate,
}

/// Order candidates best-first: exact-in prefers more output, exact-out less input; a
/// priceable candidate always beats an unpriceable one; `strategy_hash` breaks ties for a
/// snapshot-order-independent result.
fn best_first(a: &Scored, b: &Scored, exact_in: bool) -> Ordering {
    let by_score = match (a.score, b.score) {
        (Some(x), Some(y)) if exact_in => y.cmp(&x),
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    by_score.then_with(|| {
        a.candidate
            .key
            .strategy_hash
            .cmp(&b.candidate.key.strategy_hash)
    })
}

/// Build one eligible candidate, or `None` when the strategy can't source this pair:
/// an unsupported curve, an empty reserve side, or no deliverable room left.
fn build_candidate(
    strategy: &MakerStrategy,
    caps: &AvailableSnapshot,
    token_in: Address,
    token_out: Address,
) -> Option<Candidate> {
    let CurveSpec::Priceable { curve, fees_in_bps } = &strategy.curve else {
        return None;
    };
    let balance_in = strategy.balance(&token_in);
    let balance_out = strategy.balance(&token_out);
    if balance_in.is_zero() || balance_out.is_zero() {
        return None;
    }
    let cap = frozen_cap(strategy, caps, token_out);
    if cap.deliverable.is_zero() {
        return None;
    }
    Some(Candidate {
        key: strategy.key,
        token_in,
        token_out,
        cap_out: cap.deliverable,
        wallet_cap: cap.wallet,
        pool: CurvePool::from_curve(curve, token_in, token_out, balance_in, balance_out),
        fees_in_bps: fees_in_bps.clone(),
    })
}

/// A maker's two `token_out` output ceilings: the shared `wallet`, and this leg's
/// `deliverable` = `min(wallet, strategy virtual)`.
struct FrozenCap {
    deliverable: U256,
    wallet: U256,
}

fn frozen_cap(strategy: &MakerStrategy, caps: &AvailableSnapshot, token_out: Address) -> FrozenCap {
    let wallet = caps.available(&AccountKey::WalletBudget {
        maker: strategy.key.maker,
        token: token_out,
    });
    let strategy_virtual = caps.available(&AccountKey::StrategyVirtual {
        maker: strategy.key.maker,
        strategy_hash: strategy.key.strategy_hash,
        token: token_out,
    });
    FrozenCap {
        deliverable: wallet.min(strategy_virtual),
        wallet,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::registry::Curve;
    use crate::primitives::{IntentId, MakerId, StrategyHash};
    use alloy_primitives::B256;
    use std::collections::BTreeMap;

    fn request(in_tok: Address, out_tok: Address, amount: u64) -> RouteRequest {
        RouteRequest {
            intent: IntentId(B256::ZERO),
            token_in: in_tok,
            token_out: out_tok,
            amount: U256::from(amount),
            exact_in: true,
        }
    }

    fn tok(n: u8) -> Address {
        Address::from([n; 20])
    }
    fn maker(n: u8) -> MakerId {
        MakerId(Address::from([n; 20]))
    }
    fn hash(n: u8) -> StrategyHash {
        StrategyHash(B256::from([n; 32]))
    }
    fn wallet(m: MakerId, token: Address) -> AccountKey {
        AccountKey::WalletBudget { maker: m, token }
    }
    fn virt(m: MakerId, h: StrategyHash, token: Address) -> AccountKey {
        AccountKey::StrategyVirtual {
            maker: m,
            strategy_hash: h,
            token,
        }
    }

    /// An XYC strategy over `(in_tok, out_tok)` with equal reserves `bal`.
    fn xyc(
        m: MakerId,
        h: StrategyHash,
        in_tok: Address,
        out_tok: Address,
        bal: u64,
    ) -> MakerStrategy {
        let mut balances = BTreeMap::new();
        balances.insert(in_tok, U256::from(bal));
        balances.insert(out_tok, U256::from(bal));
        MakerStrategy {
            key: StrategyKey {
                maker: m,
                app: Address::ZERO,
                strategy_hash: h,
            },
            curve: CurveSpec::Priceable {
                curve: Curve::Xyc,
                fees_in_bps: vec![],
            },
            balances,
            active: true,
            program: alloy_primitives::Bytes::new(),
        }
    }

    fn caps(entries: &[(AccountKey, u64)]) -> AvailableSnapshot {
        AvailableSnapshot(entries.iter().map(|(k, v)| (*k, U256::from(*v))).collect())
    }

    /// A trade far too small to move a deep pool must not report double-digit impact.
    ///
    /// The marginal rate was probed at a millionth of the trade, which on a small trade quotes
    /// only a few whole base units — a rate wrong enough that the real one looked better than it,
    /// and the magnitude of that gap was reported as impact.
    #[test]
    fn a_dust_trade_on_a_deep_pool_has_no_measurable_impact() {
        let (in_tok, out_tok) = (tok(1), tok(2));
        let (m, h) = (maker(9), hash(9));
        let deep = 1_000_000_000_000_000_000u64;
        let snapshot = Snapshot::from_strategies([xyc(m, h, in_tok, out_tok, deep)]);
        let available = caps(&[
            (virt(m, h, out_tok), deep),
            (wallet(m, out_tok), deep),
            (virt(m, h, in_tok), deep),
            (wallet(m, in_tok), deep),
        ]);

        const SIZE: u64 = 1_000_000;
        let amount = U256::from(SIZE);
        let selection = select(&snapshot, &available, &request(in_tok, out_tok, SIZE), 4);
        let out = selection.chosen[0]
            .net_quote_exact_in(amount)
            .expect("a dust trade prices against a pool this deep");

        let impact = price_impact_pct(&selection.chosen, amount, out);
        assert!(impact < 0.1, "dust trade reported {impact}% impact");
    }

    #[test]
    fn freezes_cap_as_min_of_the_two_ceilings() {
        let (m, h) = (maker(1), hash(1));
        let (a, b) = (tok(1), tok(2));
        let snap = Snapshot::from_strategies([xyc(m, h, a, b, 1000)]);
        // Payout token is b; wallet 100, strategy-virtual 60 ⇒ cap is the tighter 60.
        let c = caps(&[(wallet(m, b), 100), (virt(m, h, b), 60)]);
        let out = select(&snap, &c, &request(a, b, 100), 64).chosen;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].cap_out, U256::from(60u64));
        assert_eq!(out[0].token_out, b);
    }

    #[test]
    fn drops_unsupported_zero_cap_and_empty_reserve() {
        let (a, b) = (tok(1), tok(2));
        let mut unsupported = xyc(maker(1), hash(1), a, b, 1000);
        unsupported.curve = CurveSpec::Unsupported;
        let zero_cap = xyc(maker(2), hash(2), a, b, 1000);
        let mut empty = xyc(maker(3), hash(3), a, b, 1000);
        empty.balances.insert(b, U256::ZERO);
        let snap = Snapshot::from_strategies([unsupported, zero_cap, empty]);
        // Ample room everywhere except maker(2)'s virtual ceiling (0) — so only the
        // eligibility rules, not the caps, decide who survives.
        let c = caps(&[
            (wallet(maker(1), b), 100),
            (virt(maker(1), hash(1), b), 100),
            (wallet(maker(2), b), 100),
            (virt(maker(2), hash(2), b), 0),
            (wallet(maker(3), b), 100),
            (virt(maker(3), hash(3), b), 100),
        ]);
        assert!(select(&snap, &c, &request(a, b, 100), 64).chosen.is_empty());
    }

    #[test]
    fn ranks_by_estimated_output_then_hash() {
        let (a, b) = (tok(1), tok(2));
        // Deeper pools quote more output for the same trade ⇒ rank higher; equal-depth
        // pools tie and break on strategy_hash asc. maker order disagrees with depth.
        let snap = Snapshot::from_strategies([
            xyc(maker(1), hash(10), a, b, 1000), // shallow
            xyc(maker(2), hash(5), a, b, 5000),  // deep
            xyc(maker(3), hash(9), a, b, 5000),  // deep, ties with maker(2)
        ]);
        let big = 1_000_000u64;
        let c = caps(&[
            (wallet(maker(1), b), big),
            (virt(maker(1), hash(10), b), big),
            (wallet(maker(2), b), big),
            (virt(maker(2), hash(5), b), big),
            (wallet(maker(3), b), big),
            (virt(maker(3), hash(9), b), big),
        ]);
        let order: Vec<_> = select(&snap, &c, &request(a, b, 100), 64)
            .chosen
            .iter()
            .map(|c| c.key.strategy_hash)
            .collect();
        assert_eq!(order, vec![hash(5), hash(9), hash(10)]);
    }

    #[test]
    fn truncates_to_k_and_returns_all_below_k() {
        let (a, b) = (tok(1), tok(2));
        // depth n·1000 ⇒ deeper ranks higher.
        let snap = Snapshot::from_strategies(
            (1u8..=5).map(|n| xyc(maker(n), hash(n), a, b, u64::from(n) * 1000)),
        );
        let mut entries = Vec::new();
        for n in 1u8..=5 {
            entries.push((wallet(maker(n), b), 1_000_000));
            entries.push((virt(maker(n), hash(n), b), 1_000_000));
        }
        let c = caps(&entries);
        let top2 = select(&snap, &c, &request(a, b, 100), 2).chosen;
        assert_eq!(top2.len(), 2);
        assert_eq!(top2[0].key.strategy_hash, hash(5)); // deepest
        assert_eq!(top2[1].key.strategy_hash, hash(4));
        assert_eq!(select(&snap, &c, &request(a, b, 100), 64).chosen.len(), 5);
    }

    #[test]
    fn reports_best_omitted_spot_only_when_truncating() {
        let (a, b) = (tok(1), tok(2));
        let snap = Snapshot::from_strategies(
            (1u8..=3).map(|n| xyc(maker(n), hash(n), a, b, u64::from(n) * 1000)),
        );
        let big = 1_000_000u64;
        let c = caps(
            &(1u8..=3)
                .flat_map(|n| {
                    [
                        (wallet(maker(n), b), big),
                        (virt(maker(n), hash(n), b), big),
                    ]
                })
                .collect::<Vec<_>>(),
        );
        // k below the eligible count ⇒ the certificate carries the best dropped pool's spot.
        assert!(select(&snap, &c, &request(a, b, 100), 2)
            .best_omitted_spot
            .is_some());
        // k at or above the count ⇒ nothing dropped, nothing to certify.
        assert!(select(&snap, &c, &request(a, b, 100), 3)
            .best_omitted_spot
            .is_none());
    }
}
