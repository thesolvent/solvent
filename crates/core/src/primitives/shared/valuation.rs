//! USD and basis-point valuations for P&L and margins. Token amounts stay wei-exact `U256`
//! (chain-exact); only these semantic valuations use `Decimal`.

use rust_decimal::Decimal;

/// A USD valuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Usd(pub Decimal);

/// Basis points, stored as the point count (`Bps(30)` = 0.30%).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Bps(pub Decimal);

/// A USD price per whole token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UsdPrice(pub Decimal);

impl UsdPrice {
    /// USD value of `whole_tokens` at this price, or `None` if the price is non-positive
    /// (a non-positive price cannot value anything).
    pub fn value(self, whole_tokens: Decimal) -> Option<Usd> {
        if self.0 <= Decimal::ZERO {
            return None;
        }
        Some(Usd(whole_tokens * self.0))
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
