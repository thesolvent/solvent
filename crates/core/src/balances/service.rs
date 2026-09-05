//! Wallet balances: the connected wallet's holdings across the whole catalog, for Swap ("~$", Max)
//! and Create ("bal / 50%"). Composes the balances oracle (on-chain balance + pullable) with the
//! asset manager (the catalog set + each token's decimals).

use std::sync::Arc;

use alloy_primitives::{Address, U256};

use crate::asset::AssetManager;
use crate::deps::balances::BalancesOracle;
use crate::primitives::amount::Amount;
use crate::primitives::asset::Token;
use crate::primitives::balances::TokenBalance;
use crate::SolventError;

pub struct BalancesService {
    oracle: Arc<dyn BalancesOracle>,
    assets: Arc<AssetManager>,
}

impl BalancesService {
    pub fn new(oracle: Arc<dyn BalancesOracle>, assets: Arc<AssetManager>) -> Self {
        Self { oracle, assets }
    }

    /// Every catalog token's `balance` + `pullable` for `owner`, from one batched read. A token the
    /// owner has never held reads as zero, so the picker always shows the full catalog.
    pub async fn balances(&self, owner: Address) -> Result<Vec<TokenBalance>, SolventError> {
        let tokens: Vec<Token> = self
            .assets
            .list(false)
            .into_iter()
            .map(|asset| Token {
                address: asset.address,
                chain_id: asset.chain_id,
                symbol: asset.symbol,
                decimals: asset.decimals,
            })
            .collect();
        let addresses: Vec<Address> = tokens.iter().map(|token| token.address).collect();
        let holdings = self.oracle.holdings(owner, &addresses).await?;
        Ok(tokens
            .into_iter()
            .map(|token| {
                let (balance, pullable) = holdings
                    .get(&token.address)
                    .map(|h| (h.balance, h.pullable))
                    .unwrap_or((U256::ZERO, U256::ZERO));
                TokenBalance {
                    balance: Amount::from_base_units(balance, token.decimals),
                    pullable: Amount::from_base_units(pullable, token.decimals),
                    token,
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deps::balances::BalancesOracleError;
    use crate::primitives::asset::{TokenList, TokenMeta};
    use crate::primitives::balances::Holdings;
    use crate::registry::SharedSnapshot;
    use std::collections::BTreeMap;

    fn addr(n: u8) -> Address {
        Address::from([n; 20])
    }

    fn meta(n: u8, symbol: &str, decimals: u8) -> TokenMeta {
        TokenMeta {
            chain_id: 31337,
            address: addr(n),
            symbol: symbol.to_string(),
            name: symbol.to_string(),
            decimals,
            logo_uri: None,
            tags: vec![],
        }
    }

    /// Returns the configured holdings for whichever queried tokens it knows; the rest are absent.
    struct FakeOracle(BTreeMap<Address, (U256, U256)>);

    #[async_trait::async_trait]
    impl BalancesOracle for FakeOracle {
        async fn holdings(
            &self,
            _owner: Address,
            tokens: &[Address],
        ) -> Result<BTreeMap<Address, Holdings>, BalancesOracleError> {
            Ok(tokens
                .iter()
                .filter_map(|token| {
                    self.0.get(token).map(|(balance, pullable)| {
                        (
                            *token,
                            Holdings {
                                balance: *balance,
                                pullable: *pullable,
                            },
                        )
                    })
                })
                .collect())
        }
    }

    fn service(oracle: FakeOracle) -> BalancesService {
        let list = TokenList {
            name: "test".to_string(),
            tokens: vec![meta(1, "USDC", 6), meta(2, "WETH", 18)],
        };
        let assets = Arc::new(AssetManager::new(list, Arc::new(SharedSnapshot::default())));
        BalancesService::new(Arc::new(oracle), assets)
    }

    #[tokio::test]
    async fn lists_the_whole_catalog_with_pullable_and_zero_fallback() {
        // The owner holds USDC (100, but only 40 pullable — allowance-capped) and no WETH.
        let oracle = FakeOracle(BTreeMap::from([(
            addr(1),
            (U256::from(100_000_000u64), U256::from(40_000_000u64)),
        )]));
        let out = service(oracle).balances(addr(9)).await.unwrap();

        assert_eq!(out.len(), 2, "the full catalog, held or not");
        let usdc = out.iter().find(|b| b.token.symbol == "USDC").unwrap();
        assert_eq!(usdc.balance.display, "100");
        assert_eq!(usdc.pullable.display, "40"); // allowance binds below the balance
        let weth = out.iter().find(|b| b.token.symbol == "WETH").unwrap();
        assert_eq!(weth.balance.display, "0"); // never held → zero, still listed
        assert_eq!(weth.pullable.display, "0");
    }
}
