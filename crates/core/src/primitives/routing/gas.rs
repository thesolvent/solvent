//! Per-leg gas cost, expressed in the **spread token**'s base units — the token the resolver's
//! profit is measured in (`token_out` for exact-in, `token_in` for exact-out) — as the sparsity
//! threshold the solver charges each leg. Pure: the adapter fetches `gas_price` (RPC) and the
//! prices (oracle) and calls this. Prices are a fast estimate, never a settlement figure.

use alloy_primitives::U256;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

use crate::primitives::UsdPrice;

/// `gas_units × gas_price` (native wei) valued in USD at `native_price`, converted to the
/// spread token at `token_price`, scaled by its decimals. Saturates to `U256::MAX`
/// (⇒ never worth splitting) on a non-positive token price or an out-of-range result.
pub fn per_leg_cost(
    gas_units: u64,
    gas_price_wei: u128,
    native_price: UsdPrice,
    token_price: UsdPrice,
    token_decimals: u8,
) -> U256 {
    if token_price.0 <= Decimal::ZERO {
        return U256::MAX;
    }
    let native_whole = Decimal::from(gas_units) * Decimal::from(gas_price_wei) / wei_per_native();
    let usd = native_whole * native_price.0;
    let token_whole = usd / token_price.0;
    token_whole
        .checked_mul(pow10(token_decimals))
        .map(|d| d.floor())
        .and_then(|d| d.to_u128())
        .map_or(U256::MAX, U256::from)
}

fn wei_per_native() -> Decimal {
    Decimal::from(1_000_000_000_000_000_000u64)
}

fn pow10(n: u8) -> Decimal {
    (0..n).fold(Decimal::ONE, |d, _| d * Decimal::from(10u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usd(n: u64) -> UsdPrice {
        UsdPrice(Decimal::from(n))
    }

    #[test]
    fn converts_native_gas_into_spread_token_units() {
        // 150k gas × 20 gwei = 0.003 native; native $3000 ⇒ $9.
        // token $1, 18 decimals ⇒ 9e18 base units.
        let cost = per_leg_cost(150_000, 20_000_000_000, usd(3000), usd(1), 18);
        assert_eq!(
            cost,
            U256::from(9u64) * U256::from(10u64).pow(U256::from(18u64))
        );
        // Same $9 in a 6-decimal $1 token ⇒ 9e6 base units.
        assert_eq!(
            per_leg_cost(150_000, 20_000_000_000, usd(3000), usd(1), 6),
            U256::from(9_000_000u64)
        );
        // A pricier output token ⇒ fewer base units for the same gas ($9 / $9000 = 0.001).
        assert_eq!(
            per_leg_cost(150_000, 20_000_000_000, usd(3000), usd(9000), 18),
            U256::from(10u64).pow(U256::from(15u64))
        );
    }

    #[test]
    fn non_positive_token_price_never_splits() {
        assert_eq!(
            per_leg_cost(
                150_000,
                20_000_000_000,
                usd(3000),
                UsdPrice(Decimal::ZERO),
                18
            ),
            U256::MAX
        );
    }
}
