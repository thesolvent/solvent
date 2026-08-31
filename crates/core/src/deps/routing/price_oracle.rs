//! The price-oracle port: a token's USD price, for converting a native gas cost into
//! output-token units. A fast off-chain estimate (e.g. Binance) — never a settlement price.

use alloy_primitives::Address;
use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::UsdPrice;

#[async_trait]
pub trait PriceOracle: Send + Sync {
    /// USD price of one whole `token`.
    async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError>;
}

/// A price-oracle failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PriceOracleError {
    /// No price is available for the token.
    #[error("no price for token `{0}`")]
    NotFound(Address),
    /// The oracle request failed.
    #[error("price-oracle request: {0}")]
    Source(String),
}
