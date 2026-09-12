use alloy_primitives::{B256, U256};
use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::crosschain::PreparedStep;
use crate::primitives::CrossChainOrderId;

#[derive(Clone, PartialEq, Eq)]
pub struct CctpPreparedStep {
    pub step: PreparedStep,
    pub message_id: B256,
}

impl core::fmt::Debug for CctpPreparedStep {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("CctpPreparedStep")
            .field("step", &self.step)
            .field("message_id", &self.message_id)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CctpQuoteTerms {
    pub max_fee: U256,
    pub finality_threshold: u32,
}

#[derive(Debug, Error)]
pub enum CctpCompletionError {
    #[error("CCTP attestation is not ready")]
    Pending,
    #[error("CCTP attestation rate limited; retry after {retry_after_secs:?} seconds")]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("CCTP completion transport: {0}")]
    Transport(String),
    #[error("invalid CCTP completion: {0}")]
    Invalid(String),
}

/// Converts an authenticated Circle message into the destination call after the origin burn exists.
#[async_trait]
pub trait CctpCompletion: Send + Sync {
    async fn quote_terms(&self, repayment: U256) -> Result<CctpQuoteTerms, CctpCompletionError>;

    async fn close_step(
        &self,
        order_id: CrossChainOrderId,
        origin_transaction: B256,
    ) -> Result<CctpPreparedStep, CctpCompletionError>;
}
