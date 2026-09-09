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

/// A water-fill allocation across maker legs: the legs to fill, the totals
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

/// Which amount is fixed by the request and which side must be recomputed exactly.
#[derive(Clone, Copy)]
enum FillKind {
    ExactIn,
    ExactOut,
}

impl FillKind {
    fn from_request(request: &RouteRequest) -> Self {
        match request.exact_in {
            true => Self::ExactIn,
            false => Self::ExactOut,
        }
    }

    fn toward_target(self, quote: &LimitedQuote) -> U256 {
        match self {
            Self::ExactIn => quote.amount_in,
            Self::ExactOut => quote.amount_out,
        }
    }

    fn is_no_worse(self, candidate: &Split, current: &Split, per_leg_cost: U256) -> bool {
        match self {
            Self::ExactIn => candidate.net_output(per_leg_cost) >= current.net_output(per_leg_cost),
            Self::ExactOut => {
                candidate.gross_input(per_leg_cost) <= current.gross_input(per_leg_cost)
            }
        }
    }
}

/// A leg that could not price at a level — contributes nothing.
const NO_FILL: LimitedQuote = LimitedQuote {
    amount_in: U256::ZERO,
    amount_out: U256::ZERO,
    limited: true,
};

/// Candidate indices that share one maker wallet, plus that wallet's frozen output budget.
struct MakerGroup {
    indices: Vec<usize>,
    wallet_cap: U256,
}

/// Stable maker groups and the reverse mapping used while exact-in assembly tops up a leg.
struct MakerGroups {
    groups: Vec<MakerGroup>,
    group_by_leg: Vec<usize>,
}

impl MakerGroups {
    fn new(candidates: &[Candidate]) -> Self {
        let mut by_maker = std::collections::BTreeMap::<MakerId, MakerGroup>::new();
        for (index, candidate) in candidates.iter().enumerate() {
            let group = by_maker
                .entry(candidate.key.maker)
                .or_insert_with(|| MakerGroup {
                    indices: Vec::new(),
                    wallet_cap: candidate.wallet_cap,
                });
            group.wallet_cap = group.wallet_cap.min(candidate.wallet_cap);
            group.indices.push(index);
        }

        let mut group_by_leg = vec![0; candidates.len()];
        let groups = by_maker
            .into_values()
            .enumerate()
            .map(|(group_index, group)| {
                for &index in &group.indices {
                    group_by_leg[index] = group_index;
                }
                group
            })
            .collect();
        Self {
            groups,
            group_by_leg,
        }
    }

    fn iter(&self) -> impl Iterator<Item = &MakerGroup> {
        self.groups.iter()
    }

    fn output_for_leg(&self, outputs: &[U256], leg_index: usize) -> U256 {
        self.groups[self.group_by_leg[leg_index]]
            .indices
            .iter()
            .fold(U256::ZERO, |sum, &index| sum.saturating_add(outputs[index]))
    }
}

/// The frozen inputs of one solve: the candidate venues, their per-leg input `bounds`, and
/// their wallet `floors`. Every fill is a pure function of a water level.
struct Book<'a> {
    candidates: &'a [Candidate],
    bounds: Vec<U256>,
    floors: Vec<Ratio>,
    groups: MakerGroups,
    kind: FillKind,
}

impl<'a> Book<'a> {
    fn new(candidates: &'a [Candidate], target: U256, kind: FillKind) -> Self {
        let bounds = input_bounds(candidates, target, kind);
        let groups = MakerGroups::new(candidates);
        let floors = wallet_floors(candidates, &bounds, &groups);
        Self {
            candidates,
            bounds,
            floors,
            groups,
            kind,
        }
    }

    /// Leg `i`'s net fill at `level`, raised to its wallet floor; a price failure fills zero.
    fn leg(&self, i: usize, level: &Ratio) -> LimitedQuote {
        let level = std::cmp::max(&self.floors[i], level);
        self.candidates[i]
            .net_quote_with_limit(self.bounds[i], level)
            .unwrap_or(NO_FILL)
    }

    /// Every leg's fill at `level`. Assembly (which runs once) uses this; the bisection uses
    /// [`measure`](Self::measure) instead, to stay allocation-free.
    fn fills(&self, level: &Ratio) -> Vec<LimitedQuote> {
        (0..self.candidates.len())
            .map(|i| self.leg(i, level))
            .collect()
    }

    /// Total toward the target at `level` — input for exact-in, output for exact-out.
    fn measure(&self, level: &Ratio) -> U256 {
        (0..self.candidates.len()).fold(U256::ZERO, |sum, i| {
            sum.saturating_add(self.kind.toward_target(&self.leg(i, level)))
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
        let used = self.groups.output_for_leg(outs, i);
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

/// A snapshot's capped liquidity at any marginal price. Preparing wallet floors once lets depth
/// sample the routing frontier without solving a new target-sized trade at every price.
pub(crate) struct Liquidity<'a> {
    candidates: &'a [Candidate],
    bounds: Vec<U256>,
    wallet_boundaries: Vec<WalletBoundary>,
    best_price: Option<Ratio>,
}

struct WalletBoundary {
    binding_level: Ratio,
    capped_outputs: Vec<(usize, U256)>,
}

impl<'a> Liquidity<'a> {
    pub(crate) fn new(candidates: &'a [Candidate]) -> Self {
        let bounds = candidates.iter().map(Candidate::full_input_bound).collect();
        let mut liquidity = Self {
            candidates,
            bounds,
            wallet_boundaries: Vec::new(),
            best_price: None,
        };
        let full = liquidity.outputs(&Ratio::zero());
        liquidity.best_price = candidates
            .iter()
            .zip(&full)
            .filter(|(_, output)| !output.is_zero())
            .filter_map(|(candidate, _)| candidate.marginal_price().ok())
            .filter(|price| !price.is_zero())
            .max();
        let mut outputs = vec![U256::ZERO; candidates.len()];
        let groups = MakerGroups::new(candidates);
        for group in groups.iter() {
            if !exceeds_budget(
                group.indices.iter().map(|&index| full[index]),
                group.wallet_cap,
            ) {
                continue;
            }
            let bracket = bracket_level(
                |level| {
                    group.indices.iter().fold(U256::ZERO, |sum, &index| {
                        sum.saturating_add(liquidity.output(index, level))
                    })
                },
                group.wallet_cap,
                None,
                price_upper(group.indices.iter().map(|&index| &candidates[index])),
            );
            for &index in &group.indices {
                outputs[index] = liquidity.output(index, &bracket.lo);
            }
            // Integer fills can jump across the wallet limit. Retain the reaching allocation
            // and trim exact outputs, instead of discarding the whole group at the next level.
            trim_to_total(
                candidates,
                &mut outputs,
                group.wallet_cap,
                group.indices.iter().copied(),
            );
            polish_wallet_fill(candidates, &group.indices, &mut outputs, group.wallet_cap);
            liquidity.wallet_boundaries.push(WalletBoundary {
                binding_level: bracket.hi,
                capped_outputs: group
                    .indices
                    .iter()
                    .map(|&index| (index, outputs[index]))
                    .collect(),
            });
        }
        liquidity
    }

    pub(crate) fn best_price(&self) -> Option<Ratio> {
        self.best_price.clone()
    }

    fn output(&self, index: usize, level: &Ratio) -> U256 {
        let candidate = &self.candidates[index];
        candidate
            .net_quote_with_limit(self.bounds[index], level)
            .map_or(U256::ZERO, |fill| fill.amount_out.min(candidate.cap_out))
    }

    fn outputs(&self, level: &Ratio) -> Vec<U256> {
        (0..self.candidates.len())
            .map(|i| self.output(i, level))
            .collect()
    }

    pub(crate) fn at_price(&self, level: &Ratio) -> Option<Split> {
        let mut outputs = vec![None; self.candidates.len()];
        for boundary in &self.wallet_boundaries {
            if level <= &boundary.binding_level {
                for &(index, output) in &boundary.capped_outputs {
                    outputs[index] = Some(output);
                }
            }
        }
        let outputs: Vec<_> = outputs
            .into_iter()
            .enumerate()
            .map(|(index, output)| output.unwrap_or_else(|| self.output(index, level)))
            .collect();
        build_representable(self.candidates, outputs, level)
    }
}

/// Build the largest representable point at this price. A depth frontier can exceed `U256` even
/// though smaller trades are executable; reduce only that synthetic endpoint, never a routed trade.
fn build_representable(
    candidates: &[Candidate],
    mut outputs: Vec<U256>,
    level: &Ratio,
) -> Option<Split> {
    if exceeds_budget(outputs.iter().copied(), U256::MAX) {
        trim_to_total(candidates, &mut outputs, U256::MAX, 0..candidates.len());
    }
    if let Some(split) = build(candidates, &outputs, FillKind::ExactOut, level) {
        return Some(split);
    }

    let mut inputs: Vec<_> = candidates
        .iter()
        .zip(&outputs)
        .map(|(candidate, &output)| match output.is_zero() {
            true => Some(U256::ZERO),
            false => candidate.net_quote_exact_out(output).ok(),
        })
        .collect::<Option<_>>()?;
    trim_to_total(candidates, &mut inputs, U256::MAX, 0..candidates.len());
    for ((candidate, input), output) in candidates.iter().zip(inputs).zip(&mut outputs) {
        if input.is_zero() || output.is_zero() {
            *output = U256::ZERO;
            continue;
        }
        *output = (*output).min(candidate.net_quote_exact_in(input).ok()?);
    }
    build(candidates, &outputs, FillKind::ExactOut, level)
}

/// Whole-input rounding can make one venue cheaper than the continuous wallet allocation.
/// Compare those endpoints once; this bounded polish does not solve the integer partition problem.
fn polish_wallet_fill(
    candidates: &[Candidate],
    indices: &[usize],
    outputs: &mut [U256],
    wallet: U256,
) {
    let mut cost = indices
        .iter()
        .try_fold(U256::ZERO, |sum, &i| {
            if outputs[i].is_zero() {
                return Some(sum);
            }
            sum.checked_add(candidates[i].net_quote_exact_out(outputs[i]).ok()?)
        })
        .unwrap_or(U256::MAX);
    let mut selected: Option<usize> = None;
    for &i in indices {
        if candidates[i].cap_out < wallet {
            continue;
        }
        let Ok(input) = candidates[i].net_quote_exact_out(wallet) else {
            continue;
        };
        if input.is_zero() || candidates[i].net_quote_exact_in(input).is_err() {
            continue;
        }
        if input < cost
            || (input == cost
                && selected.is_some_and(|previous| candidates[i].key < candidates[previous].key))
        {
            cost = input;
            selected = Some(i);
        }
    }
    if let Some(selected) = selected {
        for &i in indices {
            outputs[i] = U256::ZERO;
        }
        outputs[selected] = wallet;
    }
}

/// A water-fill allocation of `request`, or `None` when the prepared liquidity cannot meet it.
/// Bisection and rounding polish approximate the continuous optimum without globally optimizing
/// integer allocations; `warm` is the last λ for this pair and only tightens the search.
pub fn solve(
    candidates: &[Candidate],
    request: &RouteRequest,
    warm: Option<&Ratio>,
) -> Option<Split> {
    if candidates.is_empty() || request.amount.is_zero() {
        return None;
    }
    let target = request.amount;
    let kind = FillKind::from_request(request);
    let book = Book::new(candidates, target, kind);
    // λ=0 fills every venue to its cap; if that can't reach the target, there is no route.
    if book.measure(&Ratio::zero()) < target {
        return None;
    }
    let level = bracket_level(
        |l| book.measure(l),
        target,
        warm,
        price_upper(candidates.iter()),
    );
    // exact-in assembles at the under-filling side then adds the remainder; exact-out at the
    // over-filling side then trims the overshoot.
    match kind {
        FillKind::ExactIn => assemble_exact_in(&book, &level.hi, target),
        FillKind::ExactOut => assemble_exact_out(&book, &level.lo, target),
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
    upper: Ratio,
) -> LevelBracket {
    let tol = (target / U256::from(1_000_000u64)).max(U256::from(1u64));
    let mut lo = Ratio::zero();
    let mut hi = match seed.filter(|seed| !seed.is_zero()) {
        Some(seed) => seed.doubled().min(upper.clone()),
        None => upper.clone(),
    };
    let mut m_lo = fill(&lo);
    let mut m_hi = fill(&hi);
    if m_hi >= target {
        hi = upper;
        m_hi = fill(&hi);
    }
    // XYC spot ratios and Pegged's finite differences cannot exceed U256::MAX:
    // reserves fit U256 and each numerical difference divides by at least one input atom.
    if m_hi >= target {
        hi = Ratio::from(U256::MAX).doubled();
        m_hi = fill(&hi);
    }
    for _ in 0..MAX_ITERS {
        if m_lo.saturating_sub(m_hi) <= tol {
            break;
        }
        let mid = lo.midpoint(&hi);
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
fn wallet_floors(candidates: &[Candidate], bounds: &[U256], groups: &MakerGroups) -> Vec<Ratio> {
    let mut floors = vec![Ratio::zero(); candidates.len()];
    for group in groups.iter() {
        if group.indices.len() < 2 {
            continue; // a lone leg is already within its wallet via `cap_out`
        }
        // A maker's candidates share one `WalletBudget`; bind only if full fill would exceed it.
        if !exceeds_budget(
            group.indices.iter().map(|&index| {
                candidates[index]
                    .net_quote_with_limit(bounds[index], &Ratio::zero())
                    .map_or(U256::ZERO, |quote| quote.amount_out)
            }),
            group.wallet_cap,
        ) {
            continue;
        }
        let floor = group_floor_level(candidates, bounds, group);
        for &index in &group.indices {
            floors[index] = floor.clone();
        }
    }
    floors
}

/// A maker group's combined net output at water level `level`.
fn group_output_at_level(
    candidates: &[Candidate],
    bounds: &[U256],
    group: &MakerGroup,
    level: &Ratio,
) -> U256 {
    group.indices.iter().fold(U256::ZERO, |sum, &index| {
        let out = candidates[index]
            .net_quote_with_limit(bounds[index], level)
            .map(|q| q.amount_out)
            .unwrap_or(U256::ZERO);
        sum.saturating_add(out)
    })
}

/// The level at which a maker group's combined output falls to its `wallet` — where its legs
/// fill when the wallet binds. The under-budget (`hi`) side keeps the group within it.
fn group_floor_level(candidates: &[Candidate], bounds: &[U256], group: &MakerGroup) -> Ratio {
    bracket_level(
        |level| group_output_at_level(candidates, bounds, group, level),
        group.wallet_cap,
        None,
        price_upper(group.indices.iter().map(|&index| &candidates[index])),
    )
    .hi
}

/// Scale the bracket to this book; every finite-difference marginal is also bounded by U256.
fn price_upper<'a>(candidates: impl Iterator<Item = &'a Candidate>) -> Ratio {
    let price = candidates
        .filter_map(|candidate| candidate.marginal_price().ok())
        .filter(|price| !price.is_zero())
        .max()
        .unwrap_or_else(|| Ratio::from(U256::MAX));
    price.doubled()
}

/// Per-candidate gross-input ceiling for the fill: enough input to reach the leg's binding
/// output (the per-arm comments cover each direction). An un-priceable bound — an XYC asymptote,
/// or arithmetic beyond the fixed-point range — searches the representable input range.
fn input_bounds(candidates: &[Candidate], target: U256, kind: FillKind) -> Vec<U256> {
    candidates
        .iter()
        .map(|candidate| match kind {
            // exact-in: the box cap's input, capped by the whole request.
            FillKind::ExactIn => candidate
                .input_within_output(candidate.cap_out)
                .unwrap_or_else(|| candidate.feasible_input(target))
                .min(target),
            // exact-out, target below cap: round the input up so the feasibility measure can
            // reach the target — the assembly trims the overshoot back to it, under the cap.
            FillKind::ExactOut if target < candidate.cap_out => candidate
                .net_quote_exact_out(target)
                .ok()
                .unwrap_or_else(|| candidate.feasible_input(U256::MAX)),
            // exact-out, cap binds: step back so the leg never delivers past its cap.
            FillKind::ExactOut => candidate
                .input_within_output(candidate.cap_out)
                .unwrap_or_else(|| candidate.feasible_input(U256::MAX)),
        })
        .collect()
}

/// The saturating sum of a slice of amounts.
fn sum(amounts: &[U256]) -> U256 {
    amounts.iter().fold(U256::ZERO, |s, &a| s.saturating_add(a))
}

/// Subtracting each output from the budget also detects totals larger than U256.
fn exceeds_budget(mut outputs: impl Iterator<Item = U256>, budget: U256) -> bool {
    outputs
        .try_fold(budget, |remaining, output| remaining.checked_sub(output))
        .is_none()
}

/// Fill below `target` input at `level`, then place the remainder on the venue that buys the
/// most extra output — bounded by its box cap and its maker's wallet. Input reaches `target`
/// within the bisection tolerance and never over-spends it; a wallet-pinned book may strand a
/// sub-tolerance sliver rather than push a maker's combined output past its wallet.
fn assemble_exact_in(book: &Book, level: &Ratio, target: U256) -> Option<Split> {
    let fills = book.fills(level);
    let mut ins: Vec<U256> = fills.iter().map(|q| q.amount_in).collect();
    let mut outs: Vec<U256> = fills.iter().map(|q| q.amount_out).collect();
    let mut remainder = target.saturating_sub(sum(&ins));
    while !remainder.is_zero() {
        // Room per leg (box cap ∩ maker wallet), computed once for this pass.
        let rooms: Vec<U256> = (0..book.candidates.len())
            .map(|i| book.topup_room(&ins, &outs, i))
            .collect();
        // Pour the remainder into the leg that buys the most extra output for it; ties break
        // by strategy_hash so the choice is independent of candidate order.
        let Some(i) = (0..book.candidates.len())
            .filter(|&i| !rooms[i].is_zero())
            .max_by_key(|&i| {
                let add = remainder.min(rooms[i]);
                let here = book.candidates[i]
                    .net_quote_exact_in(ins[i])
                    .unwrap_or(U256::ZERO);
                let ahead = book.candidates[i]
                    .net_quote_exact_in(ins[i].saturating_add(add))
                    .unwrap_or(here);
                (
                    ahead.saturating_sub(here),
                    book.candidates[i].key.strategy_hash,
                )
            })
        else {
            break;
        };
        let add = remainder.min(rooms[i]);
        if add.is_zero() {
            break;
        }
        ins[i] = ins[i].saturating_add(add);
        outs[i] = book.candidates[i]
            .net_quote_exact_in(ins[i])
            .unwrap_or(outs[i]);
        remainder -= add;
    }
    build(book.candidates, &ins, FillKind::ExactIn, level)
}

/// Fill above `target` output at `level`, then trim the overshoot off the largest legs so
/// total output is exactly `target` (never under). Trimming only lowers output, so every
/// maker stays within its wallet.
fn assemble_exact_out(book: &Book, level: &Ratio, target: U256) -> Option<Split> {
    let mut outs: Vec<U256> = book.fills(level).iter().map(|q| q.amount_out).collect();
    trim_to_total(book.candidates, &mut outs, target, 0..book.candidates.len());
    build(book.candidates, &outs, FillKind::ExactOut, level)
}

fn trim_to_total(
    candidates: &[Candidate],
    amounts: &mut [U256],
    target: U256,
    indices: impl Iterator<Item = usize>,
) {
    let mut order: Vec<usize> = indices.filter(|&i| !amounts[i].is_zero()).collect();
    // Largest amount first; ties break by strategy_hash so trimming is order-independent.
    order.sort_by(|&a, &b| {
        amounts[b].cmp(&amounts[a]).then_with(|| {
            candidates[a]
                .key
                .strategy_hash
                .cmp(&candidates[b].key.strategy_hash)
        })
    });
    // Retain the last-to-trim legs first so even an overflowing total is trimmed exactly.
    let mut remaining = target;
    for &i in order.iter().rev() {
        amounts[i] = amounts[i].min(remaining);
        remaining -= amounts[i];
    }
}

/// Turn per-candidate working amounts into legs, pricing the other side exactly.
fn build(
    candidates: &[Candidate],
    amounts: &[U256],
    kind: FillKind,
    lambda: &Ratio,
) -> Option<Split> {
    let mut legs = Vec::new();
    let (mut amount_in, mut amount_out) = (U256::ZERO, U256::ZERO);
    for (c, &amount) in candidates.iter().zip(amounts) {
        if amount.is_zero() {
            continue;
        }
        let (leg_in, leg_out) = match kind {
            FillKind::ExactIn => (amount, c.net_quote_exact_in(amount).ok()?),
            FillKind::ExactOut => (c.net_quote_exact_out(amount).ok()?, amount),
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
        amount_in = amount_in.checked_add(leg_in)?;
        amount_out = amount_out.checked_add(leg_out)?;
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
///
/// `max_legs` is a best-effort target, not a hard cap: when no set within it can fill the trade,
/// the last feasible split is returned even though it exceeds `max_legs` — a valid route beats
/// declining a fillable trade.
pub fn solve_sparse(
    candidates: &[Candidate],
    request: &RouteRequest,
    per_leg_cost: U256,
    max_legs: usize,
    warm: Option<&Ratio>,
) -> Option<Split> {
    let kind = FillKind::from_request(request);
    let mut active: Vec<Candidate> = candidates.to_vec();
    let mut best = solve(&active, request, warm)?;
    while best.legs.len() > 1 {
        let over_cap = best.legs.len() > max_legs;
        // Smallest output is the drop candidate; ties break by strategy_hash for a stable choice.
        let worst = best
            .legs
            .iter()
            .min_by_key(|l| (l.amount_out, l.strategy_hash))?
            .strategy_hash;
        let Some(pos) = active.iter().position(|c| c.key.strategy_hash == worst) else {
            break;
        };
        let removed = active.swap_remove(pos);
        // Re-solve the smaller set, warm-started from this pair's current λ (dropping one leg
        // barely moves it). Keep the drop only if it improves the resolver's take, or we must
        // to meet `max_legs`; otherwise put the leg back and stop.
        match solve(&active, request, Some(&best.lambda)) {
            Some(next) if over_cap || kind.is_no_worse(&next, &best, per_leg_cost) => {
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
        let bounds = input_bounds(candidates, amount, FillKind::ExactIn);
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

    /// A fine-grained greedy min-input reference for exact-out: buy each output chunk from the
    /// leg that delivers it for the least extra input. The water-fill's optimum must spend no
    /// more (within a chunk).
    fn brute_force_in(candidates: &[Candidate], amount: U256, chunks: u64) -> U256 {
        let caps: Vec<U256> = candidates.iter().map(|c| c.cap_out).collect();
        let mut outs = vec![U256::ZERO; candidates.len()];
        let chunk = (amount / U256::from(chunks)).max(U256::from(1u64));
        let mut remaining = amount;
        while !remaining.is_zero() {
            let step = chunk.min(remaining);
            let best = (0..candidates.len())
                .filter(|&i| caps[i] > outs[i])
                .min_by_key(|&i| {
                    let add = step.min(caps[i] - outs[i]);
                    let here = candidates[i]
                        .net_quote_exact_out(outs[i])
                        .unwrap_or(U256::ZERO);
                    let ahead = candidates[i]
                        .net_quote_exact_out(outs[i] + add)
                        .unwrap_or(U256::MAX);
                    ahead.saturating_sub(here)
                });
            let Some(i) = best else { break };
            let add = step.min(caps[i] - outs[i]);
            outs[i] += add;
            remaining -= add;
        }
        candidates
            .iter()
            .zip(&outs)
            .map(|(c, &o)| {
                if o.is_zero() {
                    U256::ZERO
                } else {
                    c.net_quote_exact_out(o).unwrap_or(U256::ZERO)
                }
            })
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
    fn prepared_liquidity_keeps_independent_wallet_groups_separate() {
        let wallet = U256::from(10);
        let mut candidates: Vec<_> = (1..=32)
            .flat_map(|maker| {
                [2 * maker, 2 * maker + 1].map(move |hash| {
                    cand_for_maker(
                        maker,
                        hash,
                        xyc(U256::from(3), U256::from(1000)),
                        wallet,
                        wallet,
                        &[],
                    )
                })
            })
            .collect();
        for _ in 0..2 {
            let split = Liquidity::new(&candidates)
                .at_price(&Ratio::zero())
                .unwrap();
            assert_eq!(split.amount_out, wallet * U256::from(32));
            assert_eq!(split.amount_in, U256::from(32));
            for candidate in &candidates {
                let output = split
                    .legs
                    .iter()
                    .filter(|leg| leg.maker == candidate.key.maker)
                    .fold(U256::ZERO, |sum, leg| sum + leg.amount_out);
                assert_eq!(output, wallet);
            }
            candidates.reverse();
        }
    }

    #[test]
    fn representable_endpoint_preserves_caps_and_ignores_zero_output_venues() {
        let expensive_in = U256::from(1) << 254;
        let mut candidates: Vec<_> = (1..=5)
            .map(|hash| cand(hash, xyc(expensive_in, U256::from(2)), U256::from(1), &[]))
            .collect();
        candidates.push(cand(
            6,
            xyc(U256::from(3), U256::from(1000)),
            U256::from(10),
            &[],
        ));
        candidates.push(cand(
            7,
            CurvePool::Concentrate(ConcentratePool::from_reserves_and_bounds(
                tok(1),
                tok(2),
                U256::from(1000),
                U256::from(1000),
                one18(),
                one18(),
            )),
            U256::from(10),
            &[],
        ));

        let split = Liquidity::new(&candidates)
            .at_price(&Ratio::zero())
            .unwrap();
        assert_eq!(split.amount_out, U256::from(13));
        assert_eq!(
            split
                .legs
                .iter()
                .find(|leg| leg.strategy_hash == candidates[5].key.strategy_hash)
                .unwrap()
                .amount_out,
            U256::from(10)
        );
        for leg in &split.legs {
            let cap = candidates
                .iter()
                .find(|candidate| candidate.key.strategy_hash == leg.strategy_hash)
                .unwrap()
                .cap_out;
            assert!(leg.amount_out <= cap);
        }
    }

    #[test]
    fn prepared_liquidity_trims_a_wallet_whose_uncapped_sum_overflows() {
        let reserve = U256::from(1) << 255;
        let cap = U256::from(1) << 254;
        let candidates: Vec<_> = (1..=5)
            .map(|hash| cand_for_maker(9, hash, xyc(U256::from(1), reserve), cap, U256::MAX, &[]))
            .collect();
        let split = Liquidity::new(&candidates)
            .at_price(&Ratio::zero())
            .unwrap();
        assert_eq!(split.amount_out, U256::MAX);
        assert_eq!(
            split
                .legs
                .iter()
                .try_fold(U256::ZERO, |sum, leg| sum.checked_add(leg.amount_out)),
            Some(U256::MAX)
        );
    }

    #[test]
    fn split_totals_reject_overflow() {
        let reserve = U256::from(1) << 255;
        let cap = U256::from(1) << 254;
        let candidates: Vec<_> = (1..=5)
            .map(|hash| cand(hash, xyc(U256::from(1), reserve), cap, &[]))
            .collect();
        assert!(build(&candidates, &[cap; 5], FillKind::ExactOut, &Ratio::zero()).is_none());
        let candidates: Vec<_> = (1..=2)
            .map(|hash| cand(hash, xyc(reserve, U256::from(2)), U256::from(1), &[]))
            .collect();
        assert!(build(
            &candidates,
            &[U256::from(1); 2],
            FillKind::ExactOut,
            &Ratio::zero()
        )
        .is_none());
    }

    #[test]
    fn prepared_liquidity_scales_its_feasible_input_ceiling() {
        for bits in [120, 150] {
            let reserve = U256::from(1) << bits;
            let candidates = [cand(1, xyc(reserve, reserve), reserve, &[])];
            let liquidity = Liquidity::new(&candidates);
            let split = liquidity.at_price(&Ratio::zero()).unwrap();
            assert!(split.amount_out >= U256::from(1) << 100);
            if bits == 120 {
                assert!(split.amount_out > reserve / U256::from(2));
            }
            for leg in split.legs {
                assert!(candidates[0].net_quote_exact_in(leg.amount_in).is_ok());
            }
        }
    }

    #[test]
    fn prepared_liquidity_wallet_brackets_follow_the_books_price_scale() {
        for (reserve_in, reserve_out) in [
            (U256::from(1), U256::from(1) << 200),
            (U256::from(1) << 200, U256::from(1) << 50),
        ] {
            let wallet = reserve_out / U256::from(4) * U256::from(3);
            let candidates: Vec<_> = (1..=2)
                .map(|hash| {
                    cand_for_maker(9, hash, xyc(reserve_in, reserve_out), wallet, wallet, &[])
                })
                .collect();
            let liquidity = Liquidity::new(&candidates);
            let spot = candidates[0].marginal_price().unwrap();
            assert!(liquidity.at_price(&spot.doubled()).is_none());
            for divisor in [1, 2, 4, 16] {
                let level = spot.clone() * Ratio::new(U256::from(1), U256::from(divisor)).unwrap();
                if let Some(split) = liquidity.at_price(&level) {
                    assert!(split.amount_out <= wallet);
                }
            }
        }
    }

    #[test]
    fn prepared_liquidity_uses_a_cheaper_single_leg_wallet_fill() {
        let wallet = U256::from(5);
        let mut candidates = vec![
            cand_for_maker(
                9,
                1,
                xyc(U256::from(30), U256::from(100)),
                wallet,
                wallet,
                &[],
            ),
            cand_for_maker(
                9,
                2,
                xyc(U256::from(1), U256::from(10)),
                wallet,
                wallet,
                &[],
            ),
        ];
        for _ in 0..2 {
            let split = Liquidity::new(&candidates)
                .at_price(&Ratio::zero())
                .unwrap();
            assert_eq!(split.amount_out, wallet);
            assert_eq!(split.amount_in, U256::from(1));
            candidates.reverse();
        }
    }

    #[test]
    fn prepared_liquidity_preserves_discrete_shared_wallet_capacity() {
        for wallet in [10, 15, 20, 25] {
            let candidates: Vec<_> = (1..=2)
                .map(|hash| {
                    cand_for_maker(
                        9,
                        hash,
                        xyc(U256::from(3), U256::from(1000)),
                        U256::from(10),
                        U256::from(wallet),
                        &[],
                    )
                })
                .collect();
            let liquidity = Liquidity::new(&candidates);
            let split = liquidity
                .at_price(&Ratio::zero())
                .expect("partial exact-out capacity remains executable");
            assert_eq!(split.amount_out, U256::from(wallet.min(20)));
            assert!(split
                .legs
                .iter()
                .all(|leg| leg.amount_out <= U256::from(10)));
            for leg in split.legs {
                assert_eq!(leg.amount_in, U256::from(1));
            }
        }
    }

    #[test]
    fn prepared_liquidity_never_exceeds_wallet_between_search_bounds() {
        let wallet = e18(1200);
        let candidates: Vec<_> = (1..=5)
            .map(|hash| cand_for_maker(9, hash, xyc(e18(1000), e18(1000)), e18(1000), wallet, &[]))
            .collect();
        let liquidity = Liquidity::new(&candidates);
        let floor = &liquidity.wallet_boundaries[0].binding_level;
        let just_above = floor.clone() + Ratio::new(U256::from(1), e18(1)).unwrap();
        for level in [Ratio::zero(), floor.clone(), just_above] {
            let split = liquidity.at_price(&level).unwrap();
            assert!(
                split.amount_out <= wallet,
                "{} exceeds wallet {wallet}",
                split.amount_out
            );
        }
    }

    #[test]
    fn prepared_liquidity_backs_off_unrepresentable_full_cap_input() {
        let reserve = U256::from(1) << 120;
        let candidates = [cand(1, xyc(reserve, reserve), reserve - U256::from(1), &[])];
        let full_input = candidates[0]
            .net_quote_exact_out(candidates[0].cap_out)
            .unwrap();
        assert!(candidates[0].net_quote_exact_in(full_input).is_err());
        let split = Liquidity::new(&candidates)
            .at_price(&Ratio::zero())
            .unwrap();
        assert!(!split.amount_out.is_zero());
        assert!(split.amount_out <= candidates[0].cap_out);
        assert_eq!(
            split.amount_in,
            candidates[0].net_quote_exact_out(split.amount_out).unwrap()
        );
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
    fn exact_out_within_epsilon_of_brute_force_heterogeneous() {
        // Same heterogeneous set, exact-out: the optimum spends no more input than the greedy
        // min-input reference (within a chunk), and delivers the target exactly.
        let cs = [
            cand(1, xyc(e18(2000), e18(2000)), e18(10_000), &[3_000_000]), // 0.3%
            cand(2, concentrate(e18(2000)), e18(10_000), &[]),
            cand(3, pegged(e18(2000)), e18(10_000), &[1_000_000]), // 0.1%
        ];
        let amount = e18(500);
        let sol = solve(&cs, &request(amount, false), None).unwrap();
        let brute = brute_force_in(&cs, amount, 2000);
        let eps = amount / U256::from(1000u64);
        assert!(
            sol.amount_in <= brute.saturating_add(eps),
            "water-fill in {} vs brute in {}",
            sol.amount_in,
            brute
        );
        assert_eq!(sol.amount_out, amount, "delivers exactly the target");
        assert!(sol.legs.len() >= 2, "a heterogeneous split");
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
        // fragments, and high per-leg gas (in input units) collapses it — exact-out prices gas
        // through `gross_input`.
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

    #[test]
    fn sparsity_returns_a_valid_route_when_it_cannot_meet_max_legs() {
        // max_legs is a best-effort target, not a hard cap: two shallow pools each capped below
        // the target need both legs, so max_legs = 1 cannot hold the trade. Dropping to one leg
        // makes `solve` return None, and `solve_sparse` restores the leg — returning a valid
        // over-cap route rather than declining a fillable trade.
        let cs = [
            cand(1, xyc(e18(500), e18(500)), e18(400), &[]),
            cand(2, xyc(e18(500), e18(500)), e18(400), &[]),
        ];
        let target = e18(600);
        let sol = solve_sparse(&cs, &request(target, false), U256::ZERO, 1, None).unwrap();
        assert_eq!(sol.amount_out, target, "still delivers the target");
        assert_eq!(
            sol.legs.len(),
            2,
            "keeps both legs — neither fills the target alone"
        );
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

    fn split(legs: usize, amount_in: U256, amount_out: U256) -> Split {
        let leg = RouteLeg {
            maker: MakerId(Address::ZERO),
            strategy_hash: StrategyHash(B256::ZERO),
            token_in: tok(1),
            token_out: tok(2),
            amount_in: U256::ZERO,
            amount_out: U256::ZERO,
        };
        Split {
            legs: vec![leg; legs],
            amount_in,
            amount_out,
            lambda: Ratio::zero(),
        }
    }

    #[test]
    fn no_worse_than_charges_gas_per_leg_both_directions() {
        let cost = e18(1);
        // Same totals, fewer legs ⇒ less gas ⇒ better for the resolver, both directions:
        // exact-in nets more output, exact-out grosses less input.
        let one = split(1, e18(100), e18(90));
        let two = split(2, e18(100), e18(90));
        assert!(FillKind::ExactIn.is_no_worse(&one, &two, cost)); // net_output 89 ≥ 88
        assert!(!FillKind::ExactIn.is_no_worse(&two, &one, cost));
        assert!(FillKind::ExactOut.is_no_worse(&one, &two, cost)); // gross_input 101 ≤ 102
        assert!(!FillKind::ExactOut.is_no_worse(&two, &one, cost));
    }
}
