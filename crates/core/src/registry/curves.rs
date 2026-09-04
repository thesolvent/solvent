//! Closed-form Rust ports of the three Aqua swap curves — `XYCSwap`,
//! `XYCConcentrate`, and `PeggedSwap` — bit-exact to the SwapVM v1.0.1 source
//! (`@1inch/swap-vm/src/instructions/{XYCSwap,XYCConcentrate,PeggedSwap}.sol`).
//!
//! A [`Pricing`] pool is oriented for a single `tokenIn -> tokenOut` direction,
//! built from the maker's real reserves plus the strategy program's parameters.
//! `quote_exact_in` / `quote_exact_out` reproduce the router's on-chain `quote()`
//! result exactly, including every revert path (surfaced as [`CurveError`]) — so
//! the registry can price entirely off-chain with no RPC on the hot path.
//!
//! Faithfulness rests on matching Solidity's integer semantics: OpenZeppelin
//! `Math.mulDiv` (512-bit intermediate, floor), `Math.ceilDiv`, `Math.sqrt`
//! (floor and ceil), and 0.8.x checked `*`/`+`/`-` (revert on over/underflow).

use alloy_primitives::{Address, U256, U512};
use thiserror::Error;

use crate::primitives::pricing::{LimitedQuote, Ratio};
use crate::primitives::registry::{Curve, PeggedParams};

/// A revert produced by the on-chain curve, mirrored so the port can be
/// differential-fuzzed for exact parity. Which variant surfaces is diagnostic
/// only — parity is defined as "the port errs iff the contract reverts".
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum CurveError {
    /// Both maker reserves must be non-zero for the constant-product leg.
    #[error("empty maker reserves")]
    EmptyReserves,
    /// Requested output meets/exceeds the reserve, or a settlement subtraction
    /// underflowed (the trade is not fillable at this size).
    #[error("amount too large for reserves")]
    AmountTooLarge,
    /// Pegged input is below the curve's feasibility floor (`c < √u + a·u`).
    #[error("input infeasible for pegged invariant")]
    Infeasible,
    /// Pegged quadratic has no admissible root.
    #[error("no pegged solution")]
    NoSolution,
    /// A fixed-point product/sum exceeded 256 bits (a Solidity checked-math revert).
    #[error("fixed-point overflow")]
    Overflow,
    /// Division by zero (a degenerate parameter set).
    #[error("division by zero")]
    DivByZero,
    /// Price bounds or rates are outside the curve's valid domain.
    #[error("invalid curve parameters")]
    InvalidParams,
}

/// One maker curve, oriented for a fixed `tokenIn -> tokenOut` direction.
pub trait Pricing {
    /// Output for a taker selling `amount_in` of `tokenIn` (SwapVM `isExactIn`).
    fn quote_exact_in(&self, amount_in: U256) -> Result<U256, CurveError>;
    /// Input required for a taker buying `amount_out` of `tokenOut` (`!isExactIn`).
    fn quote_exact_out(&self, amount_out: U256) -> Result<U256, CurveError>;
    /// Fill input up to `amount_in`, stopping once the marginal output-per-input
    /// drops to `limit` — the water-fill step. `limited` marks a price-bound stop.
    /// Default bisects the input via `quote_exact_in`; closed-form curves override.
    fn quote_with_limit(&self, amount_in: U256, limit: &Ratio) -> Result<LimitedQuote, CurveError> {
        fill_to_limit_numerical(self, amount_in, limit)
    }
}

/// Bisect the input for the largest fill whose marginal output-per-input stays at or
/// above `limit`, using an approximate finite-difference marginal; a drained pool prices
/// at zero. For curves with no closed-form inverse (Pegged).
fn fill_to_limit_numerical<P: Pricing + ?Sized>(
    pool: &P,
    amount_in: U256,
    limit: &Ratio,
) -> Result<LimitedQuote, CurveError> {
    if amount_in.is_zero() {
        return Ok(LimitedQuote {
            amount_in: U256::ZERO,
            amount_out: U256::ZERO,
            limited: false,
        });
    }
    let step = (amount_in / U256::from(1_000_000u64)).max(U256::from(1u64));
    let marginal = |a: U256| -> Result<Ratio, CurveError> {
        let here = pool.quote_exact_in(a)?;
        match pool.quote_exact_in(a.saturating_add(step)) {
            Ok(ahead) => Ratio::new(ahead.saturating_sub(here), step).ok_or(CurveError::DivByZero),
            Err(_) => Ok(Ratio::zero()),
        }
    };
    if marginal(amount_in)?.ge(limit) {
        let out = pool.quote_exact_in(amount_in)?;
        return Ok(LimitedQuote {
            amount_in,
            amount_out: out,
            limited: false,
        });
    }
    if marginal(U256::ZERO)?.lt(limit) {
        return Ok(LimitedQuote {
            amount_in: U256::ZERO,
            amount_out: U256::ZERO,
            limited: true,
        });
    }
    let (mut lo, mut hi) = (U256::ZERO, amount_in);
    while hi - lo > U256::from(1u64) {
        let mid = lo + (hi - lo) / U256::from(2u64);
        match marginal(mid)?.ge(limit) {
            true => lo = mid,
            false => hi = mid,
        }
    }
    let out = pool.quote_exact_in(lo)?;
    Ok(LimitedQuote {
        amount_in: lo,
        amount_out: out,
        limited: lo < amount_in,
    })
}

/// `1e18` — sqrt-price fixed-point basis (`XYCConcentrate.ONE`).
#[inline]
fn one18() -> U256 {
    U256::from(1_000_000_000_000_000_000u64)
}

/// `1e27` — pegged fixed-point basis (`PeggedSwapMath.ONE`).
#[inline]
fn one27() -> U256 {
    U256::from(1_000_000_000_000_000_000u64) * U256::from(1_000_000_000u64)
}

/// Solidity `a * b` — reverts (here: `Overflow`) if the product exceeds 256 bits.
#[inline]
fn cmul(a: U256, b: U256) -> Result<U256, CurveError> {
    a.checked_mul(b).ok_or(CurveError::Overflow)
}

/// Solidity `a + b` — reverts on 256-bit overflow.
#[inline]
fn cadd(a: U256, b: U256) -> Result<U256, CurveError> {
    a.checked_add(b).ok_or(CurveError::Overflow)
}

/// Floor division `a / b` (Solidity `/`), rejecting a zero divisor.
#[inline]
fn fdiv(a: U256, b: U256) -> Result<U256, CurveError> {
    if b.is_zero() {
        return Err(CurveError::DivByZero);
    }
    Ok(a / b)
}

/// OpenZeppelin `Math.ceilDiv(a, b)` = `a == 0 ? 0 : (a - 1) / b + 1`.
#[inline]
fn ceil_div(a: U256, b: U256) -> Result<U256, CurveError> {
    if b.is_zero() {
        return Err(CurveError::DivByZero);
    }
    if a.is_zero() {
        return Ok(U256::ZERO);
    }
    Ok((a - U256::from(1u64)) / b + U256::from(1u64))
}

/// OpenZeppelin `Math.mulDiv(a, b, denom)` — full 512-bit intermediate, floored,
/// reverting (`Overflow`) when the quotient does not fit 256 bits (and on zero
/// divisor). Unlike Solidity `a * b / denom`, the intermediate never overflows.
#[inline]
fn mul_div(a: U256, b: U256, denom: U256) -> Result<U256, CurveError> {
    if denom.is_zero() {
        return Err(CurveError::DivByZero);
    }
    let quotient = (U512::from(a) * U512::from(b)) / U512::from(denom);
    if quotient > U512::from(U256::MAX) {
        return Err(CurveError::Overflow);
    }
    let limbs = quotient.as_limbs();
    Ok(U256::from_limbs([limbs[0], limbs[1], limbs[2], limbs[3]]))
}

/// OpenZeppelin `Math.sqrt(x)` — integer floor square root.
#[inline]
fn sqrt_floor(x: U256) -> U256 {
    x.root(2)
}

/// OpenZeppelin `Math.sqrt(x, Rounding.Ceil)`.
#[inline]
fn sqrt_ceil(x: U256) -> U256 {
    let r = x.root(2);
    match r * r < x {
        true => r + U256::from(1u64),
        false => r,
    }
}

/// SwapVM fee denominator (`Fee.BPS`, 1e9 = 100%).
#[inline]
fn bps_denominator() -> U256 {
    U256::from(1_000_000_000u64)
}

/// `Fee._flatFeeAmountInXD` exact-in: the maker's cut off the input before the
/// curve — `amount − ceilDiv(amount·bps, BPS)`.
pub fn apply_flat_fee_in(amount: U256, fee_bps: u32) -> Result<U256, CurveError> {
    let cut = ceil_div(cmul(amount, U256::from(fee_bps))?, bps_denominator())?;
    amount.checked_sub(cut).ok_or(CurveError::AmountTooLarge)
}

/// `Fee._flatFeeAmountInXD` exact-out: the input grossed up to cover the fee —
/// `amount + ceilDiv(amount·bps, BPS − bps)`.
pub fn apply_flat_fee_out(amount: U256, fee_bps: u32) -> Result<U256, CurveError> {
    let denominator = bps_denominator()
        .checked_sub(U256::from(fee_bps))
        .filter(|d| !d.is_zero())
        .ok_or(CurveError::InvalidParams)?;
    cadd(
        amount,
        ceil_div(cmul(amount, U256::from(fee_bps))?, denominator)?,
    )
}

/// Shrink a gross input by a strategy's stacked flat fees, in program order (exact-in).
pub fn shrink_by_fees(amount: U256, fees: &[u32]) -> Result<U256, CurveError> {
    fees.iter()
        .try_fold(amount, |a, &bps| apply_flat_fee_in(a, bps))
}

/// Gross a curve input up to cover a strategy's stacked flat fees, in reverse (exact-out).
pub fn gross_up_by_fees(amount: U256, fees: &[u32]) -> Result<U256, CurveError> {
    fees.iter()
        .rev()
        .try_fold(amount, |a, &bps| apply_flat_fee_out(a, bps))
}

/// Full-range constant-product pool (`AquaXYCAmmStrategy`).
#[derive(Debug, Clone, Copy)]
pub struct XycPool {
    balance_in: U256,
    balance_out: U256,
}

impl XycPool {
    /// From the maker's real reserves oriented to `(tokenIn, tokenOut)`.
    pub fn from_reserves(balance_in: U256, balance_out: U256) -> Self {
        Self {
            balance_in,
            balance_out,
        }
    }
}

impl XycPool {
    fn is_empty(&self) -> bool {
        self.balance_in.is_zero() || self.balance_out.is_zero()
    }
}

impl Pricing for XycPool {
    /// `XYCSwap._xycSwapXD` — output floors, the maker-favorable rounding.
    fn quote_exact_in(&self, amount_in: U256) -> Result<U256, CurveError> {
        if self.is_empty() {
            return Err(CurveError::EmptyReserves);
        }
        fdiv(
            cmul(amount_in, self.balance_out)?,
            cadd(self.balance_in, amount_in)?,
        )
    }

    /// The ceil inverse of `quote_exact_in` — input ceils, never under-charging.
    fn quote_exact_out(&self, amount_out: U256) -> Result<U256, CurveError> {
        if self.is_empty() {
            return Err(CurveError::EmptyReserves);
        }
        let denominator = self
            .balance_out
            .checked_sub(amount_out)
            .filter(|d| !d.is_zero())
            .ok_or(CurveError::AmountTooLarge)?;
        ceil_div(cmul(amount_out, self.balance_in)?, denominator)
    }

    /// Closed form: `marginal(a) = k/(bal_in + a)²`, so the fill boundary is
    /// `a* = ⌊√(k/limit)⌋ − bal_in`. A zero limit is "no floor" (fill all).
    fn quote_with_limit(&self, amount_in: U256, limit: &Ratio) -> Result<LimitedQuote, CurveError> {
        if self.is_empty() {
            return Err(CurveError::EmptyReserves);
        }
        if limit.is_zero() {
            return Ok(LimitedQuote {
                amount_in,
                amount_out: self.quote_exact_in(amount_in)?,
                limited: false,
            });
        }
        let spot = Ratio::new(self.balance_out, self.balance_in).ok_or(CurveError::DivByZero)?;
        if spot.le(limit) {
            return Ok(LimitedQuote {
                amount_in: U256::ZERO,
                amount_out: U256::ZERO,
                limited: !amount_in.is_zero(),
            });
        }
        let k = Ratio::from(self.balance_in) * Ratio::from(self.balance_out);
        let boundary = (k * limit.clone().invert().ok_or(CurveError::DivByZero)?)
            .floor_sqrt()
            .ok_or(CurveError::Overflow)?;
        // spot > limit ⇒ boundary ≥ balance_in; a rounding tie floors the fill to zero.
        let amount = amount_in.min(boundary.saturating_sub(self.balance_in));
        Ok(LimitedQuote {
            amount_in: amount,
            amount_out: self.quote_exact_in(amount)?,
            limited: amount < amount_in,
        })
    }
}

/// `XYCConcentrateArgsBuilder._computeL` (scale `1e18`).
fn concentrate_liquidity(
    balance_lt: U256,
    balance_gt: U256,
    sqrt_price_min: U256,
    sqrt_price_max: U256,
) -> Result<U256, CurveError> {
    let one = one18();
    let ratio = mul_div(sqrt_price_min, one, sqrt_price_max)?;
    // alpha is 0 only when sqrtMin >= sqrtMax, which would divide by zero below.
    let alpha = one.checked_sub(ratio).ok_or(CurveError::InvalidParams)?;
    if alpha.is_zero() {
        return Err(CurveError::InvalidParams);
    }
    let beta = cadd(
        mul_div(balance_lt, sqrt_price_min, one)?,
        mul_div(balance_gt, one, sqrt_price_max)?,
    )?;
    let four_alpha = cmul(U256::from(4u64), alpha)?;
    let four_ac = cmul(mul_div(four_alpha, balance_lt, one)?, balance_gt)?;
    let disc = cadd(cmul(beta, beta)?, four_ac)?;
    let numerator = cadd(beta, sqrt_floor(disc))?;
    mul_div(numerator, one, cmul(U256::from(2u64), alpha)?)
}

/// Concentrated-liquidity pool (`AquaXYCAmmStrategy.newConcentrate`). Grows the
/// real reserves into virtual reserves (`_xycConcentrateGrowLiquidity2D`), then
/// prices the amplified pool with the plain XYC leg (`ctx.runLoop()`).
#[derive(Debug, Clone, Copy)]
pub struct ConcentratePool {
    balance_in: U256,
    balance_out: U256,
    sqrt_price_min: U256,
    sqrt_price_max: U256,
    /// Whether `tokenIn` is the lower-address token — the on-chain price direction.
    in_is_lt: bool,
}

impl ConcentratePool {
    /// From reserves + the program's sqrt price bounds, oriented by the on-chain
    /// `tokenIn < tokenOut` rule. `sqrt_price_{min,max}` are `1e18` fixed-point
    /// and program-canonical (direction-independent).
    pub fn from_reserves_and_bounds(
        token_in: Address,
        token_out: Address,
        balance_in: U256,
        balance_out: U256,
        sqrt_price_min: U256,
        sqrt_price_max: U256,
    ) -> Self {
        Self {
            balance_in,
            balance_out,
            sqrt_price_min,
            sqrt_price_max,
            in_is_lt: token_in < token_out,
        }
    }

    /// The virtual XYC pool the concentrated leg actually swaps against — the real
    /// reserves grown by `_xycConcentrateGrowLiquidity2D`. Concentrate *is* XYC on
    /// these reserves, so pricing delegates to it.
    fn virtual_pool(&self) -> Result<XycPool, CurveError> {
        let one = one18();
        let (balance_lt, balance_gt) = match self.in_is_lt {
            true => (self.balance_in, self.balance_out),
            false => (self.balance_out, self.balance_in),
        };
        let liquidity = concentrate_liquidity(
            balance_lt,
            balance_gt,
            self.sqrt_price_min,
            self.sqrt_price_max,
        )?;
        let (add_in, add_out) = match self.in_is_lt {
            true => (
                ceil_div(cmul(liquidity, one)?, self.sqrt_price_max)?,
                mul_div(liquidity, self.sqrt_price_min, one)?,
            ),
            false => (
                ceil_div(cmul(liquidity, self.sqrt_price_min)?, one)?,
                mul_div(liquidity, one, self.sqrt_price_max)?,
            ),
        };
        Ok(XycPool::from_reserves(
            cadd(self.balance_in, add_in)?,
            cadd(self.balance_out, add_out)?,
        ))
    }
}

impl Pricing for ConcentratePool {
    fn quote_exact_in(&self, amount_in: U256) -> Result<U256, CurveError> {
        self.virtual_pool()?.quote_exact_in(amount_in)
    }

    fn quote_exact_out(&self, amount_out: U256) -> Result<U256, CurveError> {
        self.virtual_pool()?.quote_exact_out(amount_out)
    }

    fn quote_with_limit(&self, amount_in: U256, limit: &Ratio) -> Result<LimitedQuote, CurveError> {
        self.virtual_pool()?.quote_with_limit(amount_in, limit)
    }
}

/// `PeggedSwapMath.invariant(u, v, a)` = `√(u·ONE) + √(v·ONE) + a·(u+v)/ONE`.
fn pegged_invariant(u: U256, v: U256, a: U256) -> Result<U256, CurveError> {
    let one = one27();
    let sqrt_u = sqrt_floor(cmul(u, one)?);
    let sqrt_v = sqrt_floor(cmul(v, one)?);
    let linear = fdiv(cmul(a, cadd(u, v)?)?, one)?;
    cadd(cadd(sqrt_u, sqrt_v)?, linear)
}

/// `PeggedSwapMath.invariantFromReserves`.
fn pegged_invariant_from_reserves(
    x: U256,
    y: U256,
    x0: U256,
    y0: U256,
    a: U256,
) -> Result<U256, CurveError> {
    let one = one27();
    let u = fdiv(cmul(x, one)?, x0)?;
    let v = fdiv(cmul(y, one)?, y0)?;
    pegged_invariant(u, v, a)
}

/// `PeggedSwapMath.solve(u, a, c)` — the analytic √-linear root (p=0.5), using
/// the rationalized quadratic form that stays stable as `a -> 0`.
fn pegged_solve(u: U256, a: U256, invariant_c: U256) -> Result<U256, CurveError> {
    let one = one27();
    let sqrt_u = sqrt_floor(cmul(u, one)?);
    let au = fdiv(cmul(a, u)?, one)?;
    let sqrt_u_plus_au = cadd(sqrt_u, au)?;
    if invariant_c < sqrt_u_plus_au {
        return Err(CurveError::Infeasible);
    }
    let right_side = invariant_c - sqrt_u_plus_au;

    if a.is_zero() {
        return fdiv(cmul(right_side, right_side)?, one);
    }

    let four_a_right = fdiv(cmul(cmul(U256::from(4u64), a)?, right_side)?, one)?;
    let discriminant = cadd(one, four_a_right)?;
    let sqrt_discriminant = sqrt_ceil(cmul(discriminant, one)?);
    if sqrt_discriminant < one {
        return Err(CurveError::NoSolution);
    }
    let denominator = cadd(one, sqrt_discriminant)?;
    let w = fdiv(cmul(cmul(U256::from(2u64), right_side)?, one)?, denominator)?;
    fdiv(cmul(w, w)?, one)
}

/// A pegged pool's current reserves in normalized units, plus the invariant they pin.
struct Normalized {
    x0: U256,
    y0: U256,
    invariant: U256,
}

/// Pegged / stableswap pool (`AquaPeggedAmmStrategy`), oriented to `(in, out)`.
#[derive(Debug, Clone, Copy)]
pub struct PeggedPool {
    balance_in: U256,
    balance_out: U256,
    x0_init: U256,
    y0_init: U256,
    linear_width: U256,
    rate_in: U256,
    rate_out: U256,
}

impl PeggedPool {
    /// From reserves + program params, applying the on-chain `tokenIn < tokenOut`
    /// rate/normalization assignment (`PeggedSwapArgsBuilder.parseRatesAndBalances`).
    pub fn from_reserves_and_params(
        token_in: Address,
        token_out: Address,
        balance_in: U256,
        balance_out: U256,
        params: PeggedParams,
    ) -> Self {
        let (rate_in, rate_out, x0_init, y0_init) = match token_in < token_out {
            true => (params.rate_lt, params.rate_gt, params.x0, params.y0),
            false => (params.rate_gt, params.rate_lt, params.y0, params.x0),
        };
        Self {
            balance_in,
            balance_out,
            x0_init,
            y0_init,
            linear_width: params.linear_width,
            rate_in,
            rate_out,
        }
    }

    /// Normalized current reserves + the target invariant they pin.
    fn normalized(&self) -> Result<Normalized, CurveError> {
        if self.balance_in.is_zero() && self.balance_out.is_zero() {
            return Err(CurveError::EmptyReserves);
        }
        let x0 = cmul(self.balance_in, self.rate_in)?;
        let y0 = cmul(self.balance_out, self.rate_out)?;
        let invariant =
            pegged_invariant_from_reserves(x0, y0, self.x0_init, self.y0_init, self.linear_width)?;
        Ok(Normalized { x0, y0, invariant })
    }
}

impl Pricing for PeggedPool {
    fn quote_exact_in(&self, amount_in: U256) -> Result<U256, CurveError> {
        let one = one27();
        let n = self.normalized()?;
        let x1 = cadd(n.x0, cmul(amount_in, self.rate_in)?)?;
        let u1 = fdiv(cmul(x1, one)?, self.x0_init)?;
        let v1 = pegged_solve(u1, self.linear_width, n.invariant)?;
        let y1 = ceil_div(cmul(v1, self.y0_init)?, one)?;
        let delta_out = n.y0.checked_sub(y1).ok_or(CurveError::AmountTooLarge)?;
        fdiv(delta_out, self.rate_out)
    }

    fn quote_exact_out(&self, amount_out: U256) -> Result<U256, CurveError> {
        let one = one27();
        let n = self.normalized()?;
        let y1 =
            n.y0.checked_sub(cmul(amount_out, self.rate_out)?)
                .ok_or(CurveError::AmountTooLarge)?;
        let v1 = fdiv(cmul(y1, one)?, self.y0_init)?;
        let u1 = pegged_solve(v1, self.linear_width, n.invariant)?;
        let x1 = ceil_div(cmul(u1, self.x0_init)?, one)?;
        let delta_in = x1.checked_sub(n.x0).ok_or(CurveError::AmountTooLarge)?;
        ceil_div(delta_in, self.rate_in)
    }
}

/// Any priceable Aqua curve, oriented in→out. Static dispatch keeps the router's
/// hot path allocation-free (`Copy`, no `Box<dyn Pricing>`) and `Send + Sync`.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum CurvePool {
    Xyc(XycPool),
    Concentrate(ConcentratePool),
    Pegged(PeggedPool),
}

impl CurvePool {
    /// Build the oriented pool for a `token_in -> token_out` swap from a decoded
    /// curve and the maker's reserves. Shared by registry pricing and the router.
    pub fn from_curve(
        curve: &Curve,
        token_in: Address,
        token_out: Address,
        balance_in: U256,
        balance_out: U256,
    ) -> Self {
        match curve {
            Curve::Xyc => CurvePool::Xyc(XycPool::from_reserves(balance_in, balance_out)),
            Curve::Concentrate {
                sqrt_price_min,
                sqrt_price_max,
            } => CurvePool::Concentrate(ConcentratePool::from_reserves_and_bounds(
                token_in,
                token_out,
                balance_in,
                balance_out,
                *sqrt_price_min,
                *sqrt_price_max,
            )),
            Curve::Pegged(params) => CurvePool::Pegged(PeggedPool::from_reserves_and_params(
                token_in,
                token_out,
                balance_in,
                balance_out,
                *params,
            )),
        }
    }
}

/// Forward a `Pricing` call to whichever curve the pool wraps — the variant list
/// lives here once.
macro_rules! dispatch {
    ($self:ident, $method:ident $(, $arg:expr)*) => {
        match $self {
            CurvePool::Xyc(p) => p.$method($($arg),*),
            CurvePool::Concentrate(p) => p.$method($($arg),*),
            CurvePool::Pegged(p) => p.$method($($arg),*),
        }
    };
}

impl Pricing for CurvePool {
    fn quote_exact_in(&self, amount_in: U256) -> Result<U256, CurveError> {
        dispatch!(self, quote_exact_in, amount_in)
    }

    fn quote_exact_out(&self, amount_out: U256) -> Result<U256, CurveError> {
        dispatch!(self, quote_exact_out, amount_out)
    }

    fn quote_with_limit(&self, amount_in: U256, limit: &Ratio) -> Result<LimitedQuote, CurveError> {
        dispatch!(self, quote_with_limit, amount_in, limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e18(n: u64) -> U256 {
        U256::from(n) * one18()
    }

    fn lo() -> Address {
        Address::from([0x11u8; 20])
    }
    fn hi() -> Address {
        Address::from([0x22u8; 20])
    }

    #[test]
    fn mul_div_matches_floor_semantics() {
        assert_eq!(mul_div(e18(1), e18(1), one18()).unwrap(), e18(1));
        // 7 * 3 / 2 = 10 (floor of 10.5).
        assert_eq!(
            mul_div(U256::from(7u64), U256::from(3u64), U256::from(2u64)).unwrap(),
            U256::from(10u64)
        );
        // Intermediate exceeds 256 bits but the quotient fits — must not overflow.
        assert_eq!(
            mul_div(U256::MAX, U256::from(4u64), U256::from(4u64)).unwrap(),
            U256::MAX
        );
        assert_eq!(
            mul_div(U256::MAX, U256::from(2u64), U256::from(1u64)),
            Err(CurveError::Overflow)
        );
        assert_eq!(
            mul_div(U256::from(1u64), U256::from(1u64), U256::ZERO),
            Err(CurveError::DivByZero)
        );
    }

    #[test]
    fn ceil_div_and_sqrt() {
        assert_eq!(
            ceil_div(U256::from(10u64), U256::from(3u64)).unwrap(),
            U256::from(4u64)
        );
        assert_eq!(
            ceil_div(U256::from(9u64), U256::from(3u64)).unwrap(),
            U256::from(3u64)
        );
        assert_eq!(ceil_div(U256::ZERO, U256::from(3u64)).unwrap(), U256::ZERO);
        assert_eq!(sqrt_floor(U256::from(15u64)), U256::from(3u64));
        assert_eq!(sqrt_floor(U256::from(16u64)), U256::from(4u64));
        assert_eq!(sqrt_ceil(U256::from(16u64)), U256::from(4u64));
        assert_eq!(sqrt_ceil(U256::from(17u64)), U256::from(5u64));
    }

    #[test]
    fn xyc_matches_hand_computed() {
        let pool = XycPool::from_reserves(e18(1000), e18(1000));
        let out = pool.quote_exact_in(e18(100)).unwrap();
        assert_eq!(out, e18(100) * e18(1000) / (e18(1000) + e18(100)));
        assert_eq!(out, U256::from(90_909_090_909_090_909_090u128));

        // Exact-out is the ceil inverse and never under-charges the maker.
        let needed = pool.quote_exact_out(out).unwrap();
        assert!(needed <= e18(100));
        assert_eq!(
            pool.quote_exact_out(e18(1000)),
            Err(CurveError::AmountTooLarge)
        );
    }

    #[test]
    fn xyc_empty_reserves_revert() {
        let pool = XycPool::from_reserves(U256::ZERO, e18(1000));
        assert_eq!(pool.quote_exact_in(e18(1)), Err(CurveError::EmptyReserves));
    }

    #[test]
    fn concentrate_beats_full_range_in_band() {
        let reserve = e18(1000);
        let amount = e18(100);
        let sqrt_min = sqrt_floor((one18() / U256::from(2u64)) * one18()); // price 0.5
        let sqrt_max = sqrt_floor((U256::from(2u64) * one18()) * one18()); // price 2.0

        let xyc = XycPool::from_reserves(reserve, reserve);
        let conc = ConcentratePool::from_reserves_and_bounds(
            lo(),
            hi(),
            reserve,
            reserve,
            sqrt_min,
            sqrt_max,
        );

        let xyc_out = xyc.quote_exact_in(amount).unwrap();
        let conc_out = conc.quote_exact_in(amount).unwrap();
        assert!(
            conc_out > xyc_out,
            "concentration must amplify depth in-band"
        );
    }

    #[test]
    fn pegged_beats_full_range_near_peg() {
        // 18/18 decimals -> rates = 1, x0 = y0 = reserve.
        let reserve = e18(1000);
        let amount = e18(100);
        let params = PeggedParams {
            x0: reserve,
            y0: reserve,
            linear_width: U256::from(100u64) * one27(),
            rate_lt: U256::from(1u64),
            rate_gt: U256::from(1u64),
        };
        let pegged = PeggedPool::from_reserves_and_params(lo(), hi(), reserve, reserve, params);
        let xyc = XycPool::from_reserves(reserve, reserve);

        let peg_out = pegged.quote_exact_in(amount).unwrap();
        let xyc_out = xyc.quote_exact_in(amount).unwrap();
        assert!(peg_out > xyc_out, "pegged must beat XYC for a stable pair");
        // Stays within ~1% of 1:1 in the flat region, and never over-pays out.
        assert!(peg_out > amount - amount / U256::from(100u64));
        assert!(peg_out <= amount);
    }

    fn ratio(n: u64, d: u64) -> Ratio {
        Ratio::new(U256::from(n), U256::from(d)).unwrap()
    }

    #[test]
    fn xyc_quote_with_limit_fills_to_the_price_bound() {
        let pool = XycPool::from_reserves(U256::from(1000u64), U256::from(1000u64));
        // Spot output-per-input is 1; fill until the marginal drops to 1/2:
        // k/(1000+a)² = 1/2 → 1000+a = √(2·10⁶) = 1414 → a = 414.
        let q = pool
            .quote_with_limit(U256::from(2000u64), &ratio(1, 2))
            .unwrap();
        assert_eq!(q.amount_in, U256::from(414u64));
        assert_eq!(
            q.amount_out,
            pool.quote_exact_in(U256::from(414u64)).unwrap()
        );
        assert!(q.limited, "the price bound, not the input, ended the fill");

        // A floor at or above spot takes nothing.
        let none = pool
            .quote_with_limit(U256::from(2000u64), &ratio(1, 1))
            .unwrap();
        assert_eq!(none.amount_in, U256::ZERO);
        assert!(none.limited);

        // A floor the fill never reaches consumes all the input.
        let all = pool
            .quote_with_limit(U256::from(100u64), &ratio(1, 1000))
            .unwrap();
        assert_eq!(all.amount_in, U256::from(100u64));
        assert_eq!(
            all.amount_out,
            pool.quote_exact_in(U256::from(100u64)).unwrap()
        );
        assert!(!all.limited);
    }

    #[test]
    fn concentrate_quote_with_limit_respects_the_price_bound() {
        let reserve = e18(1000);
        let sqrt_min = sqrt_floor((one18() / U256::from(2u64)) * one18()); // price 0.5
        let sqrt_max = sqrt_floor((U256::from(2u64) * one18()) * one18()); // price 2.0
        let pool = ConcentratePool::from_reserves_and_bounds(
            lo(),
            hi(),
            reserve,
            reserve,
            sqrt_min,
            sqrt_max,
        );
        // Symmetric reserves and band ⇒ spot output-per-input is 1; a floor at spot takes nothing.
        let none = pool.quote_with_limit(e18(100_000), &ratio(1, 1)).unwrap();
        assert_eq!(none.amount_in, U256::ZERO);
        assert!(none.limited);
        // Below spot, a large request fills partially to the bound; a tighter floor fills strictly less.
        let mid = pool.quote_with_limit(e18(100_000), &ratio(1, 2)).unwrap();
        let tight = pool.quote_with_limit(e18(100_000), &ratio(3, 4)).unwrap();
        assert!(mid.limited && tight.limited);
        assert!(tight.amount_in < mid.amount_in);
        assert_eq!(mid.amount_out, pool.quote_exact_in(mid.amount_in).unwrap());
        // A zero floor is "no floor": the whole request is consumed, unlimited.
        let all = pool.quote_with_limit(e18(100), &Ratio::zero()).unwrap();
        assert_eq!(all.amount_in, e18(100));
        assert!(!all.limited);
    }

    #[test]
    fn pegged_quote_with_limit_is_consistent_and_monotone() {
        let reserve = e18(1000);
        let params = PeggedParams {
            x0: reserve,
            y0: reserve,
            linear_width: U256::from(100u64) * one27(),
            rate_lt: U256::from(1u64),
            rate_gt: U256::from(1u64),
        };
        let pool = PeggedPool::from_reserves_and_params(lo(), hi(), reserve, reserve, params);
        // A higher price floor fills no more, and the numerical fill's output is the pool's own quote.
        let tight = pool.quote_with_limit(e18(500), &ratio(999, 1000)).unwrap();
        let loose = pool.quote_with_limit(e18(500), &ratio(990, 1000)).unwrap();
        assert!(tight.amount_in <= loose.amount_in);
        assert_eq!(
            tight.amount_out,
            pool.quote_exact_in(tight.amount_in).unwrap()
        );
        assert_eq!(
            loose.amount_out,
            pool.quote_exact_in(loose.amount_in).unwrap()
        );
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig { failure_persistence: None, ..proptest::prelude::ProptestConfig::default() })]
        // XYC structural invariants over the whole realistic range (fee-free).
        #[test]
        fn xyc_invariants(
            balance_in in 1u128..=1_000_000_000_000_000_000_000_000u128,
            balance_out in 1u128..=1_000_000_000_000_000_000_000_000u128,
            amount in 1u128..=1_000_000_000_000_000_000_000_000u128,
            extra in 0u128..=1_000_000_000_000_000_000_000_000u128,
        ) {
            let pool = XycPool::from_reserves(U256::from(balance_in), U256::from(balance_out));
            let out = pool.quote_exact_in(U256::from(amount)).expect("exact-in");
            // A constant-product pool can never be fully drained.
            proptest::prop_assert!(out < U256::from(balance_out));
            // Output is monotonic non-decreasing in the input.
            let out_more = pool
                .quote_exact_in(U256::from(amount) + U256::from(extra))
                .expect("exact-in more");
            proptest::prop_assert!(out_more >= out);
            // Exact-out never demands more than the exact-in amount that produced `out`.
            if !out.is_zero() {
                let needed = pool.quote_exact_out(out).expect("exact-out");
                proptest::prop_assert!(needed <= U256::from(amount));
            }
        }
    }
}
