//! The rebate-payer port: send a maker their recaptured rebate. One transfer per (maker, token);
//! the payout service batches across intents by summing first.

use alloy_primitives::{Address, U256};
use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::MakerId;

/// Sends a maker their recapture rebate.
#[async_trait]
pub trait RebatePayer: Send + Sync {
    /// Transfer `amount` of `token` to `maker`.
    async fn pay(
        &self,
        maker: MakerId,
        token: Address,
        amount: U256,
    ) -> Result<(), RebatePayerError>;
}

/// A rebate-payer failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RebatePayerError {
    #[error("rebate payer: {0}")]
    Send(String),
}
