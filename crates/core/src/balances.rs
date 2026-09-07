//! The balances slice: the connected wallet's per-token holdings across the whole catalog, for Swap
//! ("~$", Max) and Create ("bal / 50%"). Composes the balances oracle (on-chain balance + pullable)
//! with the asset manager (the catalog set + each token's decimals).

use std::sync::Arc;

use alloy_primitives::Address;

use crate::asset::AssetManager;
use crate::deps::balances::BalancesOracle;
use crate::valuation::Valuation;
use crate::SolventError;

pub use crate::primitives::amount::{Holdings, TokenBalance};

pub struct BalancesService {
    oracle: Arc<dyn BalancesOracle>,
    assets: Arc<AssetManager>,
    valuation: Arc<Valuation>,
}

impl BalancesService {
    pub fn new(
        oracle: Arc<dyn BalancesOracle>,
        assets: Arc<AssetManager>,
        valuation: Arc<Valuation>,
    ) -> Self {
        Self {
            oracle,
            assets,
            valuation,
        }
    }

    /// Every catalog token's `balance` + `pullable` for `owner`, from one batched read. A token the
    /// owner has never held reads as zero, so the picker always shows the full catalog.
    pub async fn balances(&self, owner: Address) -> Result<Vec<TokenBalance>, SolventError> {
        let tokens = self.assets.catalog_tokens();
        let addresses: Vec<Address> = tokens.iter().map(|token| token.address).collect();
        let holdings = self.oracle.holdings(owner, &addresses).await?;
        let mut out = Vec::with_capacity(tokens.len());
        for token in tokens {
            let holding = holdings.get(&token.address).copied().unwrap_or_default();
            out.push(TokenBalance {
                balance: self
                    .valuation
                    .amount(holding.balance, token.address, token.decimals)
                    .await,
                pullable: self
                    .valuation
                    .amount(holding.pullable, token.address, token.decimals)
                    .await,
                token,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deps::balances::BalancesOracleError;
    use crate::deps::routing::{PriceOracle, PriceOracleError};
    use crate::primitives::asset::{TokenList, TokenMeta};
    use crate::primitives::UsdPrice;
    use crate::registry::SharedSnapshot;
    use alloy_primitives::U256;
    use async_trait::async_trait;
    use rust_decimal::Decimal;
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

    #[async_trait]
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

    struct FakePrices(BTreeMap<Address, UsdPrice>);

    #[async_trait]
    impl PriceOracle for FakePrices {
        async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError> {
            self.0
                .get(&token)
                .copied()
                .ok_or(PriceOracleError::NotFound(token))
        }
    }

    fn valuation(prices: &[(Address, u64)]) -> Arc<Valuation> {
        let map = prices
            .iter()
            .map(|(a, dollars)| (*a, UsdPrice(Decimal::from(*dollars))))
            .collect();
        Arc::new(Valuation::new(Arc::new(FakePrices(map))))
    }

    fn service(oracle: FakeOracle, valuation: Arc<Valuation>) -> BalancesService {
        let list = TokenList {
            name: "test".to_string(),
            tokens: vec![meta(1, "USDC", 6), meta(2, "WETH", 18)],
        };
        let assets = Arc::new(AssetManager::new(list, Arc::new(SharedSnapshot::default())));
        BalancesService::new(Arc::new(oracle), assets, valuation)
    }

    #[tokio::test]
    async fn lists_the_whole_catalog_with_pullable_and_zero_fallback() {
        // The owner holds USDC (100, but only 40 pullable — allowance-capped) and no WETH.
        let oracle = FakeOracle(BTreeMap::from([(
            addr(1),
            (U256::from(100_000_000u64), U256::from(40_000_000u64)),
        )]));
        let out = service(oracle, valuation(&[]))
            .balances(addr(9))
            .await
            .unwrap();

        assert_eq!(out.len(), 2, "the full catalog, held or not");
        let usdc = out.iter().find(|b| b.token.symbol == "USDC").unwrap();
        assert_eq!(usdc.balance.display, "100");
        assert_eq!(usdc.pullable.display, "40"); // allowance binds below the balance
        let weth = out.iter().find(|b| b.token.symbol == "WETH").unwrap();
        assert_eq!(weth.balance.display, "0"); // never held → zero, still listed
        assert_eq!(weth.pullable.display, "0");
    }

    #[tokio::test]
    async fn values_priced_balances_in_usd() {
        let oracle = FakeOracle(BTreeMap::from([(
            addr(1),
            (U256::from(100_000_000u64), U256::from(40_000_000u64)),
        )]));
        // USDC priced at $1; WETH left unpriced.
        let out = service(oracle, valuation(&[(addr(1), 1)]))
            .balances(addr(9))
            .await
            .unwrap();

        let usdc = out.iter().find(|b| b.token.symbol == "USDC").unwrap();
        assert_eq!(usdc.balance.usd, Some(100.0));
        assert_eq!(usdc.pullable.usd, Some(40.0));
        let weth = out.iter().find(|b| b.token.symbol == "WETH").unwrap();
        assert_eq!(weth.balance.usd, None); // unpriced → no value
    }
}
