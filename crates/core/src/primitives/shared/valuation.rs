//! USD and basis-point valuations for P&L and margins. Token amounts stay wei-exact `U256`
//! (chain-exact); only these semantic valuations use `Decimal`.

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

/// A USD valuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Usd(pub Decimal);

impl Usd {
    /// The value as `f64`, for the wire (`Amount.usd`). Lossy — a display concern, never accounting.
    pub fn to_f64(self) -> f64 {
        self.0.to_f64().unwrap_or(0.0)
    }
}

/// Basis points, stored as the point count (`Bps(30)` = 0.30%).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Bps(pub Decimal);

/// A USD price per whole token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UsdPrice(pub Decimal);

impl UsdPrice {
    /// One US dollar — the price of a stablecoin at par. Seeds a pair-less quote stable (USDT) so
    /// every supported asset values.
    pub const PAR: UsdPrice = UsdPrice(Decimal::ONE);

    /// USD value of `whole_tokens` at this price, or `None` if the price is non-positive
    /// (a non-positive price cannot value anything).
    pub fn value(self, whole_tokens: Decimal) -> Option<Usd> {
        if self.0 <= Decimal::ZERO {
            return None;
        }
        Some(Usd(whole_tokens * self.0))
    }

    /// The price as `f64`, for the wire (`Asset.price_usd`). Lossy — a display concern.
    pub fn to_f64(self) -> f64 {
        self.0.to_f64().unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_guards_non_positive_price() {
        assert_eq!(UsdPrice(Decimal::ZERO).value(Decimal::from(100)), None);
        assert_eq!(UsdPrice(Decimal::from(-1)).value(Decimal::from(100)), None);
        assert_eq!(
            UsdPrice(Decimal::new(25, 1)).value(Decimal::from(4)),
            Some(Usd(Decimal::from(10)))
        );
    }
}
