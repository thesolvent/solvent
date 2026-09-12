//! The fill-store port: durable tracking of submitted, not-yet-terminal fills, so a restart
//! recovers and reconciles them rather than losing the in-flight set. The tx engine's own handle is
//! stored opaquely (serialized by the adapter that owns it) — core never inspects its bytes.

use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::execution::{ExecStatus, TrackedFill};
use crate::primitives::{IntentId, ReservationId};

#[async_trait]
pub trait FillStore: Send + Sync {
    /// Record a submitted fill: the reservation it will post or void, and the opaque engine handle
    /// to query it by. Idempotent on the intent — re-tracking is a no-op.
    async fn track(
        &self,
        intent: IntentId,
        reservation: ReservationId,
        handle: &[u8],
    ) -> Result<(), FillStoreError>;

    /// The opaque engine handle for a tracked intent, or `None` if it is not tracked.
    async fn handle(&self, intent: IntentId) -> Result<Option<Vec<u8>>, FillStoreError>;

    /// Drop a fill that reached a terminal state.
    async fn untrack(&self, intent: IntentId) -> Result<(), FillStoreError>;

    /// Every fill still tracked as in-flight — the reconcile working set, and after a restart the
    /// fills to recover.
    async fn tracked(&self) -> Result<Vec<TrackedFill>, FillStoreError>;

    /// Preserve a terminal status long enough for higher-level coordinators to observe it even
    /// after the generic reconcile loop removes the in-flight handle.
    async fn record_terminal(
        &self,
        _intent: IntentId,
        _status: &ExecStatus,
    ) -> Result<(), FillStoreError> {
        Ok(())
    }

    /// Read a previously recorded terminal status for an intent.
    async fn terminal(&self, _intent: IntentId) -> Result<Option<ExecStatus>, FillStoreError> {
        Ok(None)
    }
}

/// A fill-store failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FillStoreError {
    /// The database call or a payload conversion failed.
    #[error("db: {0}")]
    Db(String),
}
