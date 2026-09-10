//! Construction of one signed `executeRebate` transaction payload.

use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::rebate::{RebateExecution, RebatePlan};
use crate::primitives::registry::MakerStrategy;
use crate::primitives::ReservationId;

#[async_trait]
pub trait RebateCallBuilder: Send + Sync {
    async fn build(
        &self,
        strategy: &MakerStrategy,
        plan: RebatePlan,
        reservation: ReservationId,
        deadline_block: u64,
        published_at: u64,
    ) -> Result<RebateExecution, RebateCallBuilderError>;
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RebateCallBuilderError {
    #[error("rebate plan is invalid: {0}")]
    InvalidPlan(&'static str),
    #[error("rebate plan does not match the selected strategy")]
    StrategyMismatch,
    #[error("rebate strategy bytes do not match their registry hash")]
    StrategyHashMismatch,
    #[error("rebate strategy program could not be decoded")]
    UndecodableProgram,
    #[error("rebate strategy maker does not match its program")]
    StrategyMakerMismatch,
    #[error("rebate strategy is not protected by the configured taker credential")]
    UnprotectedStrategy,
    #[error("rebate authorization could not be signed")]
    Signing,
}
