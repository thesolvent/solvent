//! The chain-reading [`BalancesOracle`]: a wallet's per-token `balance` and `pullable`
//! (`min(balanceOf, allowance→Aqua)`), batched into two Multicall3 aggregates for the whole catalog.

use std::collections::BTreeMap;

use alloy::primitives::Address;
use alloy::providers::Provider;
use async_trait::async_trait;
use solvent_core::{
    deps::balances::{BalancesOracle, BalancesOracleError},
    primitives::amount::Holdings,
};

use crate::erc20::read_balance_allowances;

pub struct AlloyBalancesOracle<P> {
    provider: P,
    aqua: Address,
}

impl<P: Provider + Clone + 'static> AlloyBalancesOracle<P> {
    /// `aqua` is the allowance spender the pullable amount is measured against.
    pub fn new(provider: P, aqua: Address) -> Self {
        Self { provider, aqua }
    }
}

#[async_trait]
impl<P: Provider + Clone + 'static> BalancesOracle for AlloyBalancesOracle<P> {
    async fn holdings(
        &self,
        owner: Address,
        tokens: &[Address],
    ) -> Result<BTreeMap<Address, Holdings>, BalancesOracleError> {
        let accounts: Vec<(Address, Address)> =
            tokens.iter().map(|token| (owner, *token)).collect();
        let reads = read_balance_allowances(&self.provider, self.aqua, &accounts)
            .await
            .map_err(|e| BalancesOracleError::Chain(e.to_string()))?;
        Ok(tokens
            .iter()
            .zip(reads)
            .map(|(token, read)| {
                (
                    *token,
                    Holdings {
                        balance: read.balance,
                        pullable: read.balance.min(read.allowance),
                    },
                )
            })
            .collect())
    }
}
