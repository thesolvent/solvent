//! Shared ERC-20 reads: the `IERC20` interface plus a batched `(balanceOf, allowance→spender)` read
//! for many `(owner, token)` accounts in two Multicall3 aggregates. Reused by the budget source and
//! the balances oracle.

use alloy::primitives::{Address, U256};
use alloy::providers::{MulticallError, Provider};
use alloy::sol;

sol! {
    #[sol(rpc)]
    interface IERC20 {
        function balanceOf(address account) external view returns (uint256);
        function allowance(address owner, address spender) external view returns (uint256);
    }
}

/// A holder's on-chain balance and its allowance to a fixed spender, for one token.
pub struct BalanceAllowance {
    pub balance: U256,
    pub allowance: U256,
}

/// Read `(balanceOf(owner), allowance(owner, spender))` for every `(owner, token)` in `accounts`, in
/// two Multicall3 aggregates — all balances in one, all allowances in the other (distinct call
/// types). The result is aligned with `accounts`; empty in, empty out (no call made).
pub async fn read_balance_allowances<P: Provider + Clone>(
    provider: &P,
    spender: Address,
    accounts: &[(Address, Address)],
) -> Result<Vec<BalanceAllowance>, MulticallError> {
    if accounts.is_empty() {
        return Ok(Vec::new());
    }
    let mut balances = provider.multicall().dynamic::<IERC20::balanceOfCall>();
    let mut allowances = provider.multicall().dynamic::<IERC20::allowanceCall>();
    for (owner, token) in accounts {
        let erc20 = IERC20::new(*token, provider.clone());
        balances = balances.add_dynamic(erc20.balanceOf(*owner));
        allowances = allowances.add_dynamic(erc20.allowance(*owner, spender));
    }
    let balances = balances.aggregate().await?;
    let allowances = allowances.aggregate().await?;
    Ok(balances
        .into_iter()
        .zip(allowances)
        .map(|(balance, allowance)| BalanceAllowance { balance, allowance })
        .collect())
}
