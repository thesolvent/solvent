use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::crosschain::{ChainExecutionPlan, PreparedStep, StepValidationContext};

#[derive(Debug, Error)]
pub enum StepValidatorError {
    #[error("invalid cross-chain step: {0}")]
    Invalid(String),
}

/// Decodes contract-specific calls and binds their security-critical fields to the admitted order.
#[async_trait]
pub trait StepValidator: Send + Sync {
    async fn validate(
        &self,
        context: &StepValidationContext,
        step: &PreparedStep,
    ) -> Result<(), StepValidatorError>;

    async fn validate_plan(
        &self,
        context: &StepValidationContext,
        plan: &ChainExecutionPlan,
    ) -> Result<(), StepValidatorError> {
        for step in &plan.steps {
            self.validate(context, step).await?;
        }
        Ok(())
    }
}
