use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::crosschain::{DirectExecutionPlans, DirectOrderAuthorization};

#[derive(Debug, Error)]
pub enum DirectPlanAuthorError {
    #[error("invalid direct authorization: {0}")]
    Invalid(String),
    #[error("direct plan author is unavailable: {0}")]
    Unavailable(String),
}

#[async_trait]
pub trait DirectPlanAuthor: Send + Sync {
    async fn author(
        &self,
        authorization: &DirectOrderAuthorization,
    ) -> Result<DirectExecutionPlans, DirectPlanAuthorError>;
}
