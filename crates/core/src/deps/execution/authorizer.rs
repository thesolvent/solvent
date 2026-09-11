//! Policy-signing port for protected maker executions.

use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::execution::{ExecutionAuthorization, PolicySignature};

#[async_trait]
pub trait ExecutionAuthorizer: Send + Sync {
    async fn authorize(
        &self,
        authorization: &ExecutionAuthorization,
    ) -> Result<PolicySignature, ExecutionAuthorizerError>;
}

/// A policy signature could not be produced. Signer internals are intentionally omitted because
/// key-provider errors may include sensitive material.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExecutionAuthorizerError {
    #[error("policy signer failed")]
    Signing,
}
