//! The recapture-store port: durably accrue the per-maker rebate credits a confirmed fill earned,
//! keyed by intent so a re-driven `reconcile` never double-credits, and let the payout worker read
//! the unpaid ones and mark them settled.

use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::recapture::{AccruedCredit, RecaptureCredit};
use crate::primitives::IntentId;

/// Accrues, reads, and settles the recapture credits fills earn.
#[async_trait]
pub trait RecaptureStore: Send + Sync {
    /// Record `credits` for `intent`. Idempotent on `intent`: accruing the same intent twice (a
    /// re-driven reconcile) records it once.
    async fn accrue(
        &self,
        intent: IntentId,
        credits: &[RecaptureCredit],
    ) -> Result<(), RecaptureStoreError>;

    /// Every accrued credit not yet paid out.
    async fn outstanding(&self) -> Result<Vec<AccruedCredit>, RecaptureStoreError>;

    /// Mark `credits` paid, so a later sweep skips them. Idempotent.
    async fn mark_settled(&self, credits: &[AccruedCredit]) -> Result<(), RecaptureStoreError>;
}

/// A recapture-store failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RecaptureStoreError {
    #[error("recapture store: {0}")]
    Write(String),
}
