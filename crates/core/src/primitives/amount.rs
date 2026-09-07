//! Token-amount wire types: an amount as base units + a human display + optional USD, and the
//! shapes that pair it with a token (a roster entry, a set, a wallet holding). One representation
//! for every token amount in a response.

use alloy_primitives::U256;
use serde::Serialize;

use super::asset::Token;

/// A token amount: `raw` base units (never a float), a human-formatted `display`, and `usd` (`None`
/// until pricing is wired).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Amount {
    pub raw: String,
    pub display: String,
    pub usd: Option<f64>,
}

impl Amount {
    /// Build from a base-unit balance and the token's decimals.
    pub fn from_base_units(raw: U256, decimals: u8) -> Self {
        Self {
            display: format_units(raw, decimals),
            raw: raw.to_string(),
            usd: None,
        }
    }
}

/// One token paired with an amount.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TokenAmount {
    pub token: Token,
    pub amount: Amount,
}

/// A set of token amounts (e.g. a strategy's per-token balances) with an optional USD total.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TokenAmounts {
    pub entries: Vec<TokenAmount>,
    pub total_usd: Option<f64>,
}

/// One token a wallet holds: its on-chain `balance` and the `pullable` amount a pull could take now
/// (`min(balance, allowance→settlement)`).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TokenBalance {
    pub token: Token,
    pub balance: Amount,
    pub pullable: Amount,
}

/// The raw on-chain figures for one token, as the
/// [`BalancesOracle`](crate::deps::balances::BalancesOracle) returns them — before decimals turn
/// them into a display [`Amount`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Holdings {
    pub balance: U256,
    pub pullable: U256,
}

/// Format `raw` base units as a human decimal string, trailing zeros trimmed (`"1.5"`, `"4"`). Wraps
/// alloy's `format_units`; a nonsensical `decimals` (> 77) falls back to the raw integer string.
pub(crate) fn format_units(raw: U256, decimals: u8) -> String {
    match alloy_primitives::utils::format_units(raw, decimals) {
        Ok(s) if s.contains('.') => s.trim_end_matches('0').trim_end_matches('.').to_string(),
        Ok(s) => s,
        Err(_) => raw.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_across_decimals() {
        assert_eq!(format_units(U256::from(1_500_000u64), 6), "1.5");
        assert_eq!(format_units(U256::from(1_000_000u64), 6), "1");
        assert_eq!(format_units(U256::from(500_000u64), 6), "0.5");
        assert_eq!(format_units(U256::from(1u64), 18), "0.000000000000000001");
        assert_eq!(format_units(U256::ZERO, 6), "0");
        assert_eq!(format_units(U256::from(42u64), 0), "42");
    }

    #[test]
    fn amount_carries_raw_and_display() {
        let a = Amount::from_base_units(U256::from(2_500_000u64), 6);
        assert_eq!(a.raw, "2500000");
        assert_eq!(a.display, "2.5");
        assert_eq!(a.usd, None);
    }
}
