//! The gas-price port: the chain's current gas price in wei per gas unit. Fetched
//! periodically and cached by the adapter — off the quote hot path, it only sizes the
//! per-leg sparsity threshold.

use async_trait::async_trait;
use thiserror::Error;

#[async_trait]
pub trait GasPrice: Send + Sync {
    /// Current gas price, wei per gas unit.
    async fn gas_price_wei(&self) -> Result<u128, GasPriceError>;
}

/// A gas-price lookup failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GasPriceError {
    /// The chain read failed.
    #[error("gas-price read: {0}")]
    Source(String),
}
