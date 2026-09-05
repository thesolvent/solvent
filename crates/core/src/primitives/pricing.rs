//! Exact-rational pricing math shared by curve pricing and routing.
//!
//! `Ratio` is copied from amm-rs (`amm-core`, MIT/Apache-2.0,
//! github.com/21r21a33333/amm-rs): a reduced non-negative rational over
//! `num_rational::BigRational`, arbitrary-precision so intermediate products never
//! overflow `U256`. `floor_sqrt` is a local addition for the water-fill's
//! marginal-price inversion.

use alloy_primitives::U256;
use num_bigint::{BigInt, Sign};
use num_rational::BigRational;
use num_traits::{Signed, ToPrimitive, Zero};

/// A reduced, non-negative exact rational. Construction reduces; arithmetic never
/// overflows; structural equality is value equality.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ratio(BigRational);

impl Ratio {
    /// From `U256` parts. `None` if `den == 0`.
    #[must_use]
    pub fn new(num: U256, den: U256) -> Option<Ratio> {
        if den.is_zero() {
            None
        } else {
            Some(Ratio(BigRational::new(to_bigint(num), to_bigint(den))))
        }
    }

    /// The reciprocal. `None` if the ratio is zero.
    #[must_use]
    pub fn invert(self) -> Option<Ratio> {
        if self.0.is_zero() {
            None
        } else {
            Some(Ratio(self.0.recip()))
        }
    }

    /// The zero ratio.
    pub fn zero() -> Ratio {
        Ratio(BigRational::zero())
    }

    /// Whether the ratio equals zero.
    pub fn is_zero(&self) -> bool {
        self.0.is_zero()
    }

    /// `floor(√self)` = `floor(isqrt(num·den) / den)`. `None` only if the result
    /// exceeds `U256` (unreachable for any realistic price).
    #[must_use]
    pub fn floor_sqrt(&self) -> Option<U256> {
        let root = (self.0.numer() * self.0.denom()).sqrt() / self.0.denom();
        from_bigint(&root)
    }

    /// The greatest integer `≤ self`, as `U256`. `None` only if it exceeds `U256`.
    #[must_use]
    pub fn floor(&self) -> Option<U256> {
        from_bigint(&self.0.floor().to_integer())
    }

    /// The least integer `≥ self`, as `U256`. `None` only if it exceeds `U256`.
    #[must_use]
    pub fn ceil(&self) -> Option<U256> {
        from_bigint(&self.0.ceil().to_integer())
    }

    /// Half of this ratio — the water-fill's bisection step.
    #[must_use]
    pub fn halved(self) -> Ratio {
        Ratio(self.0 / BigInt::from(2))
    }

    /// The relative difference from `other` in basis points, floored:
    /// `|self − other| / other × 10000`. `u64::MAX` when `other` is zero (undefined) or the
    /// result exceeds `u64`. The KKT certificate's marginal-versus-λ residual.
    #[must_use]
    pub fn rel_diff_bps(&self, other: &Ratio) -> u64 {
        if other.0.is_zero() {
            return u64::MAX;
        }
        let diff = (self.0.clone() - other.0.clone()).abs();
        let bps = diff / other.0.clone() * BigRational::from_integer(BigInt::from(10_000u32));
        bps.to_integer().to_u64().unwrap_or(u64::MAX)
    }
}

impl core::ops::Mul for Ratio {
    type Output = Ratio;

    /// Multiply two ratios (reduced). Infallible with arbitrary precision.
    fn mul(self, rhs: Ratio) -> Ratio {
        Ratio(self.0 * rhs.0)
    }
}

impl From<U256> for Ratio {
    /// A whole number as a rational over 1.
    fn from(n: U256) -> Ratio {
        Ratio(BigRational::from(to_bigint(n)))
    }
}

impl core::ops::Add for Ratio {
    type Output = Ratio;

    /// Add two ratios (reduced). Infallible with arbitrary precision — the water-fill's
    /// bisection midpoint.
    fn add(self, rhs: Ratio) -> Ratio {
        Ratio(self.0 + rhs.0)
    }
}

fn to_bigint(x: U256) -> BigInt {
    BigInt::from_bytes_be(Sign::Plus, &x.to_be_bytes::<32>())
}

fn from_bigint(x: &BigInt) -> Option<U256> {
    let (sign, bytes) = x.to_bytes_be();
    if sign == Sign::Minus || bytes.len() > 32 {
        None
    } else {
        Some(U256::from_be_slice(&bytes))
    }
}

/// The outcome of a price-bounded (partial-fill) quote — amm-rs `LimitedQuote`,
/// with the tokens fixed by the pool's orientation.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LimitedQuote {
    /// Input actually consumed (below the request when `limited`).
    pub amount_in: U256,
    /// Output produced for `amount_in`.
    pub amount_out: U256,
    /// Whether the price limit stopped the fill before all input was consumed.
    pub limited: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(n: u64, d: u64) -> Ratio {
        Ratio::new(U256::from(n), U256::from(d)).unwrap()
    }

    #[test]
    fn reduces_inverts_and_orders() {
        assert_eq!(r(6, 4), r(3, 2));
        assert_eq!(r(3, 2).invert().unwrap(), r(2, 3));
        assert!(r(1, 3) < r(1, 2) && r(2, 4) == r(1, 2));
        assert!(Ratio::new(U256::from(1u64), U256::ZERO).is_none());
        assert!(r(0, 5).invert().is_none());
    }

    #[test]
    fn floor_sqrt_is_the_integer_root_of_the_rational() {
        assert_eq!(r(16, 1).floor_sqrt().unwrap(), U256::from(4u64)); // exact
        assert_eq!(r(2, 1).floor_sqrt().unwrap(), U256::from(1u64)); // √2 → 1
        assert_eq!(r(9, 4).floor_sqrt().unwrap(), U256::from(1u64)); // √2.25 → 1
        assert_eq!(r(100, 9).floor_sqrt().unwrap(), U256::from(3u64)); // √11.1 → 3
                                                                       // Product overflows U256 but the arbitrary-precision root is exact.
        let big = Ratio::new(U256::MAX, U256::from(1u64)).unwrap();
        let root = big.floor_sqrt().unwrap();
        assert!(
            root * root <= U256::MAX
                && (root + U256::from(1u64))
                    .checked_mul(root + U256::from(1u64))
                    .is_none_or(|v| v > U256::MAX)
        );
    }
}
