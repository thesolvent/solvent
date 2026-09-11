use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::crosschain::PreparedStep;

#[derive(Debug, Error)]
pub enum StepMaterializerError {
    #[error("cross-chain step is malformed: {0}")]
    Invalid(String),
    #[error("cross-chain step materialization failed: {0}")]
    Backend(String),
}

/// Refreshes chain-derived call fields, such as the current native CCIP fee, before submission.
#[async_trait]
pub trait StepMaterializer: Send + Sync {
    async fn materialize(&self, step: &PreparedStep)
        -> Result<PreparedStep, StepMaterializerError>;
}
