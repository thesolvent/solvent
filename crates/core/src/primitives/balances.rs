//! Wallet-balance value types: a token's on-chain balance and pullable amount for a queried wallet.

use alloy_primitives::U256;
use serde::Serialize;

use super::amount::Amount;
use super::asset::Token;

/// One token a wallet holds: its on-chain `balance` and the `pullable` amount a pull could take now
/// (`min(balance, allowance→settlement)`).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TokenBalance {
    pub token: Token,
    pub balance: Amount,
    pub pullable: Amount,
}

/// The raw on-chain figures for one token, as the [`BalancesOracle`](crate::deps::balances::BalancesOracle)
/// returns them — before decimals turn them into a display [`Amount`].
pub struct Holdings {
    pub balance: U256,
    pub pullable: U256,
}
