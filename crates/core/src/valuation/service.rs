//! What a token amount is worth in USD, best-effort. Turns the [`PriceOracle`] port's `Result` into
//! an `Option`: a missing, failed, or non-positive price yields no value (the FE renders `—`), and
//! never a request error. Prices are a fast off-chain estimate, never a settlement price.

use std::sync::Arc;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::deps::routing::PriceOracle;
use crate::primitives::amount::format_units;
use crate::primitives::asset::Token;
use crate::primitives::{Usd, UsdPrice};

pub struct Valuation {
    oracle: Arc<dyn PriceOracle>,
}

impl Valuation {
    pub fn new(oracle: Arc<dyn PriceOracle>) -> Self {
        Self { oracle }
    }

    /// The USD price of one whole `token`, or `None` if none is available.
    pub async fn price(&self, token: Address) -> Option<UsdPrice> {
        self.oracle.price(token).await.ok()
    }

    /// The USD value of `base_units` of `token`, or `None` if it is unpriced or unparseable.
    pub async fn usd(&self, base_units: U256, token: &Token) -> Option<Usd> {
        let whole = format_units(base_units, token.decimals)
            .parse::<Decimal>()
            .ok()?;
        self.price(token.address).await?.value(whole)
    }

    /// The USD total over several holdings, or `None` if any component is unpriced — a total that
    /// silently dropped a token would understate it.
    pub async fn tvl<'a>(&self, items: impl IntoIterator<Item = (&'a Token, U256)>) -> Option<Usd> {
        let mut total = Decimal::ZERO;
        for (token, base_units) in items {
            total += self.usd(base_units, token).await?.0;
        }
        Some(Usd(total))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use async_trait::async_trait;

    use super::*;
    use crate::deps::routing::PriceOracleError;

    struct FakeOracle(HashMap<Address, UsdPrice>);

    #[async_trait]
    impl PriceOracle for FakeOracle {
        async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError> {
            self.0
                .get(&token)
                .copied()
                .ok_or(PriceOracleError::NotFound(token))
        }
    }

    fn token(tag: u8, decimals: u8) -> Token {
        Token {
            address: Address::from([tag; 20]),
            chain_id: 1,
            symbol: String::new(),
            decimals,
        }
    }

    fn valuation(priced: &[(&Token, u64)]) -> Valuation {
        let prices = priced
            .iter()
            .map(|(t, dollars)| (t.address, UsdPrice(Decimal::from(*dollars))))
            .collect();
        Valuation::new(Arc::new(FakeOracle(prices)))
    }

    #[tokio::test]
    async fn usd_scales_by_decimals() {
        let usdc = token(1, 6);
        let weth = token(2, 18);
        let v = valuation(&[(&usdc, 1), (&weth, 2000)]);
        // 2.5 USDC @ $1 = $2.50
        assert_eq!(
            v.usd(U256::from(2_500_000u64), &usdc).await,
            Some(Usd(Decimal::new(25, 1)))
        );
        // 1 WETH @ $2000 = $2000
        assert_eq!(
            v.usd(U256::from(1_000_000_000_000_000_000u64), &weth).await,
            Some(Usd(Decimal::from(2000)))
        );
    }

    #[tokio::test]
    async fn usd_none_when_unpriced() {
        let usdc = token(1, 6);
        let unlisted = token(3, 18);
        let v = valuation(&[(&usdc, 1)]);
        assert_eq!(v.usd(U256::from(1_000_000u64), &unlisted).await, None);
    }

    #[tokio::test]
    async fn tvl_all_or_nothing() {
        let usdc = token(1, 6);
        let weth = token(2, 18);
        let unlisted = token(3, 18);
        let v = valuation(&[(&usdc, 1), (&weth, 2000)]);
        // $2.50 + $2000 = $2002.50
        assert_eq!(
            v.tvl([
                (&usdc, U256::from(2_500_000u64)),
                (&weth, U256::from(1_000_000_000_000_000_000u64)),
            ])
            .await,
            Some(Usd(Decimal::new(20025, 1)))
        );
        // one unpriced component collapses the whole total
        assert_eq!(
            v.tvl([
                (&usdc, U256::from(2_500_000u64)),
                (&unlisted, U256::from(1u64)),
            ])
            .await,
            None
        );
    }
}
