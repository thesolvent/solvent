use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::primitives::crosschain::{
    ChainExecutionPlan, LegQuote, LegQuoteRequest, Preparation, RemoteCommand, StepEvidence,
    StepValidationContext,
};
use crate::primitives::{AggregateQuoteId, CrossChainOrderId, CrossChainStepId, PrepareToken};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RemoteProgress {
    Pending,
    Finalized { evidence: StepEvidence },
    Failed { reason: String },
}

#[derive(Debug, Error)]
pub enum RemoteSolventError {
    #[error("remote Solvent unavailable: {0}")]
    Unavailable(String),
    #[error("remote Solvent rejected request: {0}")]
    Rejected(String),
    #[error("remote Solvent returned an invalid response: {0}")]
    InvalidResponse(String),
}

#[async_trait]
pub trait RemoteSolvent: Send + Sync {
    async fn quote(&self, request: &LegQuoteRequest) -> Result<LegQuote, RemoteSolventError>;
    async fn stage(
        &self,
        context: &StepValidationContext,
        plan: &ChainExecutionPlan,
    ) -> Result<(), RemoteSolventError>;
    async fn stage_cctp_completion(
        &self,
        context: &StepValidationContext,
        plan: &ChainExecutionPlan,
        preparation: PrepareToken,
    ) -> Result<(), RemoteSolventError> {
        let _ = (context, plan, preparation);
        Err(RemoteSolventError::Rejected(
            "remote Solvent does not support authorized CCTP completion staging".to_string(),
        ))
    }
    async fn prepare(
        &self,
        aggregate_id: AggregateQuoteId,
        quote: &LegQuote,
    ) -> Result<Preparation, RemoteSolventError>;
    async fn commit(&self, token: PrepareToken) -> Result<Preparation, RemoteSolventError>;
    async fn inspect(&self, token: PrepareToken) -> Result<Preparation, RemoteSolventError>;
    async fn release(&self, token: PrepareToken) -> Result<Preparation, RemoteSolventError>;
    async fn command(
        &self,
        order_id: CrossChainOrderId,
        command_id: CrossChainStepId,
        command: RemoteCommand,
        preparation: PrepareToken,
    ) -> Result<RemoteProgress, RemoteSolventError>;
}
