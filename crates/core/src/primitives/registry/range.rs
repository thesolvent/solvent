//! Turning a decoded [`Curve`] into the human price range a maker's position covers — the `range`
//! object the position and pool DTOs show. Concentrated bounds are the program's `1e18`-fixed sqrt
//! prices squared, adjusted for token decimals and oriented to `quote per base`.

use alloy_primitives::U256;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;

use super::curve::{Curve, PeggedParams};

/// Which price-range shape a position covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeKind {
    /// Full-range XYC — no price bounds.
    Full,
    /// Concentrated liquidity between `lower`/`upper`.
    Bounded,
    /// Pegged around a central rate.
    Peg,
}

/// A position's price range in human, `quote per base` terms. Prices are decimal strings (the wire
/// shape); which fields are populated is gated by `kind`.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionRange {
    pub kind: RangeKind,
    pub lower_price: Option<String>,
    pub upper_price: Option<String>,
    pub peg_price: Option<String>,
    pub below_pct: Option<f64>,
    pub above_pct: Option<f64>,
}

impl PositionRange {
    /// A range of `kind` with no bounds yet — the base for each shape's own fields.
    fn empty(kind: RangeKind) -> Self {
        Self {
            kind,
            lower_price: None,
            upper_price: None,
            peg_price: None,
            below_pct: None,
            above_pct: None,
        }
    }
}

/// The human price range for `curve`, given the pair's lower/higher-address token decimals and
/// whether the display `base` is the lower-address token. Result is `quote per base`, so the bounds
/// invert (and swap ends) when the base is the higher-address token.
pub fn price_range(curve: &Curve, dec_lo: u8, dec_hi: u8, base_is_lo: bool) -> PositionRange {
    match curve {
        Curve::Xyc => PositionRange::empty(RangeKind::Full),
        Curve::Concentrate {
            sqrt_price_min,
            sqrt_price_max,
        } => concentrated(*sqrt_price_min, *sqrt_price_max, dec_lo, dec_hi, base_is_lo),
        Curve::Pegged(params) => pegged(params, dec_lo, dec_hi, base_is_lo),
    }
}

/// One US-dollar of `1e18` fixed-point — the base the concentrate sqrt bounds and pegged marginal
/// share.
const E18: u64 = 1_000_000_000_000_000_000;

fn pegged(p: &PeggedParams, dec_lo: u8, dec_hi: u8, base_is_lo: bool) -> PositionRange {
    let band = symmetric_range_pct(p.linear_width);
    PositionRange {
        kind: RangeKind::Peg,
        lower_price: None,
        upper_price: None,
        peg_price: peg_price(p, dec_lo, dec_hi, base_is_lo),
        below_pct: band,
        above_pct: band,
    }
}

/// The central peg (marginal at the balanced point, where the curve's slopes cancel):
/// `(y0/x0)·(rate_lt/rate_gt)` is the `hi-per-lo` base-unit price, then decimals-adjusted and
/// oriented to `quote per base`. Reserve-independent — it is the peg the band sits around.
fn peg_price(p: &PeggedParams, dec_lo: u8, dec_hi: u8, base_is_lo: bool) -> Option<String> {
    // `hi-per-lo` base-unit price × 1e18 = (y0/x0)·(rate_lt/rate_gt)·1e18, in two mulDivs so the
    // intermediates stay in U256.
    let ratio = mul_div(p.y0, U256::from(E18), p.x0)?;
    let peg_1e18 = mul_div(ratio, p.rate_lt, p.rate_gt)?;
    // Fold the token decimals in U256 too — a mixed-decimals base price is huge, and only the
    // human result (≈ the peg, near 1e18) is small enough for `Decimal`.
    let human_1e18 = mul_div(peg_1e18, pow10(dec_lo)?, pow10(dec_hi)?)?;
    let human =
        Decimal::from_u128(u128::try_from(human_1e18).ok()?)?.checked_div(Decimal::from(E18))?;
    let oriented = if base_is_lo {
        human
    } else {
        Decimal::ONE.checked_div(human)?
    };
    Some(fmt(oriented))
}

fn pow10(exp: u8) -> Option<U256> {
    Some(U256::from(10u128.checked_pow(u32::from(exp))?))
}

/// The symmetric `±%` band a `linearWidth` (A, at `1e27`) encodes, matching the SDK's
/// `symmetricRangePercentFromLinearWidth`: `100 · ONE / (2A + ONE)` at the SDK's `1e13` precision.
fn symmetric_range_pct(a: U256) -> Option<f64> {
    let one = U256::from(10u128.pow(27));
    let denom = a.checked_mul(U256::from(2u64))?.checked_add(one)?;
    let x = mul_div(one, one, denom)?;
    let scaled = mul_div(x, U256::from(100u64) * U256::from(10u128.pow(13)), one)?;
    Some(u128::try_from(scaled).ok()? as f64 / 1e13)
}

fn mul_div(a: U256, b: U256, c: U256) -> Option<U256> {
    a.checked_mul(b)?.checked_div(c)
}

fn concentrated(
    sqrt_min: U256,
    sqrt_max: U256,
    dec_lo: u8,
    dec_hi: u8,
    base_is_lo: bool,
) -> PositionRange {
    let bounds = price_hi_per_lo(sqrt_min, dec_lo, dec_hi)
        .zip(price_hi_per_lo(sqrt_max, dec_lo, dec_hi))
        .and_then(|(min, max)| oriented(min, max, base_is_lo));
    PositionRange {
        kind: RangeKind::Bounded,
        lower_price: bounds.as_ref().map(|(lower, _)| fmt(*lower)),
        upper_price: bounds.as_ref().map(|(_, upper)| fmt(*upper)),
        peg_price: None,
        below_pct: None,
        above_pct: None,
    }
}

/// The human `hi per lo` price a `1e18`-fixed sqrt price encodes:
/// `(sqrt / 1e18)^2 · 10^(dec_lo - dec_hi)`. `None` if it overflows `Decimal`.
fn price_hi_per_lo(sqrt: U256, dec_lo: u8, dec_hi: u8) -> Option<Decimal> {
    let sqrt = Decimal::from(u128::try_from(sqrt).ok()?);
    let s = sqrt.checked_div(Decimal::from(E18))?;
    let base_units = s.checked_mul(s)?;
    let diff = i32::from(dec_lo) - i32::from(dec_hi);
    let factor = Decimal::from(10u64.checked_pow(diff.unsigned_abs())?);
    if diff >= 0 {
        base_units.checked_mul(factor)
    } else {
        base_units.checked_div(factor)
    }
}

/// Orient a `[min, max]` `hi-per-lo` range to `quote per base`: identity when the base is the lo
/// token, else inverted (`1/price`), which also swaps the ends.
fn oriented(min: Decimal, max: Decimal, base_is_lo: bool) -> Option<(Decimal, Decimal)> {
    if base_is_lo {
        Some((min, max))
    } else {
        Some((
            Decimal::ONE.checked_div(max)?,
            Decimal::ONE.checked_div(min)?,
        ))
    }
}

fn fmt(price: Decimal) -> String {
    price.round_dp(8).normalize().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn concentrate(sqrt_min: u128, sqrt_max: u128) -> Curve {
        Curve::Concentrate {
            sqrt_price_min: U256::from(sqrt_min),
            sqrt_price_max: U256::from(sqrt_max),
        }
    }

    #[test]
    fn xyc_is_full_range() {
        let r = price_range(&Curve::Xyc, 18, 18, true);
        assert_eq!(r.kind, RangeKind::Full);
        assert_eq!(r.lower_price, None);
        assert_eq!(r.upper_price, None);
    }

    #[test]
    fn concentrated_bounds_match_the_sdk_raw_prices() {
        // The exact sqrt bounds the SDK emits for rawPriceMin=0.5, rawPriceMax=2.0
        // (crates/core/tests/fixtures/aqua_strategies.json).
        let r = price_range(
            &concentrate(707106781186547524, 1414213562373095048),
            18,
            18,
            true,
        );
        assert_eq!(r.kind, RangeKind::Bounded);
        assert_eq!(r.lower_price.as_deref(), Some("0.5"));
        assert_eq!(r.upper_price.as_deref(), Some("2"));
    }

    #[test]
    fn concentrated_adjusts_for_decimals() {
        // raw hi-per-lo = 1.0 and 4.0 (sqrt 1e18, 2e18); lo has 12 more decimals than hi → ×1e12.
        let r = price_range(
            &concentrate(1_000_000_000_000_000_000, 2_000_000_000_000_000_000),
            18,
            6,
            true,
        );
        assert_eq!(r.lower_price.as_deref(), Some("1000000000000"));
        assert_eq!(r.upper_price.as_deref(), Some("4000000000000"));
    }

    #[test]
    fn concentrated_inverts_when_base_is_the_higher_token() {
        // hi-per-lo range [1, 4]; base is hi → quote-per-base = 1/price, ends swap → [0.25, 1].
        let r = price_range(
            &concentrate(1_000_000_000_000_000_000, 2_000_000_000_000_000_000),
            18,
            18,
            false,
        );
        assert_eq!(r.lower_price.as_deref(), Some("0.25"));
        assert_eq!(r.upper_price.as_deref(), Some("1"));
    }

    fn pegged(x0: u64, y0: u64, rate_lt: u64, rate_gt: u64, linear_width: u128) -> Curve {
        Curve::Pegged(PeggedParams {
            x0: U256::from(x0),
            y0: U256::from(y0),
            linear_width: U256::from(linear_width),
            rate_lt: U256::from(rate_lt),
            rate_gt: U256::from(rate_gt),
        })
    }

    #[test]
    fn pegged_peg_is_the_rate_scaled_ratio() {
        // peg (hi per lo) = (y0/x0)·(rate_lt/rate_gt); rates cancel here → 2.0.
        let r = price_range(&pegged(1, 2, 1, 1, 100 * 10u128.pow(27)), 18, 18, true);
        assert_eq!(r.kind, RangeKind::Peg);
        assert_eq!(r.peg_price.as_deref(), Some("2"));
        // base is the higher token → quote-per-base inverts → 0.5.
        let inv = price_range(&pegged(1, 2, 1, 1, 100 * 10u128.pow(27)), 18, 18, false);
        assert_eq!(inv.peg_price.as_deref(), Some("0.5"));
    }

    #[test]
    fn pegged_band_matches_the_sdk_formula() {
        // A = 100e27 → 100·ONE/(2A+ONE) = 100/201 ≈ 0.4975%.
        let r = price_range(&pegged(1, 1, 1, 1, 100 * 10u128.pow(27)), 18, 18, true);
        let band = r.below_pct.expect("band");
        assert!((band - 0.497_512_437_810_9).abs() < 1e-9, "band was {band}");
        assert_eq!(r.above_pct, r.below_pct);
    }
}
