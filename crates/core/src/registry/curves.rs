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

use crate::primitives::registry::PeggedParams;

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
    if r * r < x {
        r + U256::from(1u64)
    } else {
        r
    }
}

/// `XYCSwap._xycSwapXD` on the given (already virtual, for Concentrate) reserves.
/// Output floors and input ceils — the maker-favorable rounding.
fn xyc_exact_in(balance_in: U256, balance_out: U256, amount_in: U256) -> Result<U256, CurveError> {
    if balance_in.is_zero() || balance_out.is_zero() {
        return Err(CurveError::EmptyReserves);
    }
    fdiv(cmul(amount_in, balance_out)?, cadd(balance_in, amount_in)?)
}

fn xyc_exact_out(
    balance_in: U256,
    balance_out: U256,
    amount_out: U256,
) -> Result<U256, CurveError> {
    if balance_in.is_zero() || balance_out.is_zero() {
        return Err(CurveError::EmptyReserves);
    }
    let numerator = cmul(amount_out, balance_in)?;
    let denominator = balance_out
        .checked_sub(amount_out)
        .ok_or(CurveError::AmountTooLarge)?;
    if denominator.is_zero() {
        return Err(CurveError::AmountTooLarge);
    }
    ceil_div(numerator, denominator)
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

impl Pricing for XycPool {
    fn quote_exact_in(&self, amount_in: U256) -> Result<U256, CurveError> {
        xyc_exact_in(self.balance_in, self.balance_out, amount_in)
    }

    fn quote_exact_out(&self, amount_out: U256) -> Result<U256, CurveError> {
        xyc_exact_out(self.balance_in, self.balance_out, amount_out)
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

    /// The virtual `(balanceIn, balanceOut)` the XYC leg actually swaps against.
    fn virtual_reserves(&self) -> Result<(U256, U256), CurveError> {
        let one = one18();
        let (balance_lt, balance_gt) = if self.in_is_lt {
            (self.balance_in, self.balance_out)
        } else {
            (self.balance_out, self.balance_in)
        };
        let liquidity = concentrate_liquidity(
            balance_lt,
            balance_gt,
            self.sqrt_price_min,
            self.sqrt_price_max,
        )?;
        if self.in_is_lt {
            let add_in = ceil_div(cmul(liquidity, one)?, self.sqrt_price_max)?;
            let add_out = mul_div(liquidity, self.sqrt_price_min, one)?;
            Ok((
                cadd(self.balance_in, add_in)?,
                cadd(self.balance_out, add_out)?,
            ))
        } else {
            let add_in = ceil_div(cmul(liquidity, self.sqrt_price_min)?, one)?;
            let add_out = mul_div(liquidity, one, self.sqrt_price_max)?;
            Ok((
                cadd(self.balance_in, add_in)?,
                cadd(self.balance_out, add_out)?,
            ))
        }
    }
}

impl Pricing for ConcentratePool {
    fn quote_exact_in(&self, amount_in: U256) -> Result<U256, CurveError> {
        let (balance_in, balance_out) = self.virtual_reserves()?;
        xyc_exact_in(balance_in, balance_out, amount_in)
    }

    fn quote_exact_out(&self, amount_out: U256) -> Result<U256, CurveError> {
        let (balance_in, balance_out) = self.virtual_reserves()?;
        xyc_exact_out(balance_in, balance_out, amount_out)
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
        let (rate_in, rate_out, x0_init, y0_init) = if token_in < token_out {
            (params.rate_lt, params.rate_gt, params.x0, params.y0)
        } else {
            (params.rate_gt, params.rate_lt, params.y0, params.x0)
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
    fn normalized(&self) -> Result<(U256, U256, U256), CurveError> {
        if self.balance_in.is_zero() && self.balance_out.is_zero() {
            return Err(CurveError::EmptyReserves);
        }
        let x0 = cmul(self.balance_in, self.rate_in)?;
        let y0 = cmul(self.balance_out, self.rate_out)?;
        let invariant =
            pegged_invariant_from_reserves(x0, y0, self.x0_init, self.y0_init, self.linear_width)?;
        Ok((x0, y0, invariant))
    }
}

impl Pricing for PeggedPool {
    fn quote_exact_in(&self, amount_in: U256) -> Result<U256, CurveError> {
        let one = one27();
        let (x0, y0, invariant) = self.normalized()?;
        let x1 = cadd(x0, cmul(amount_in, self.rate_in)?)?;
        let u1 = fdiv(cmul(x1, one)?, self.x0_init)?;
        let v1 = pegged_solve(u1, self.linear_width, invariant)?;
        let y1 = ceil_div(cmul(v1, self.y0_init)?, one)?;
        let delta_out = y0.checked_sub(y1).ok_or(CurveError::AmountTooLarge)?;
        fdiv(delta_out, self.rate_out)
    }

    fn quote_exact_out(&self, amount_out: U256) -> Result<U256, CurveError> {
        let one = one27();
        let (x0, y0, invariant) = self.normalized()?;
        let y1 = y0
            .checked_sub(cmul(amount_out, self.rate_out)?)
            .ok_or(CurveError::AmountTooLarge)?;
        let v1 = fdiv(cmul(y1, one)?, self.y0_init)?;
        let u1 = pegged_solve(v1, self.linear_width, invariant)?;
        let x1 = ceil_div(cmul(u1, self.x0_init)?, one)?;
        let delta_in = x1.checked_sub(x0).ok_or(CurveError::AmountTooLarge)?;
        ceil_div(delta_in, self.rate_in)
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
