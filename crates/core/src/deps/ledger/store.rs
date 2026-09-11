//! The ledger store port: durable, idempotent persistence of the reservation lifecycle.
//! `open_reservations` returns the pending rows a restart replays to rebuild holds.

use alloy_primitives::U256;
use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::ledger::Reservation;
use crate::primitives::ReservationId;

#[async_trait]
pub trait LedgerStore: Send + Sync {
    /// Persist an admitted reservation as `Pending`. Idempotent on `reservation.id`
    /// (`= hash(intentId ‖ routePlanHash)`), so a duplicated command is a no-op.
    async fn reserve(&self, reservation: &Reservation) -> Result<(), LedgerStoreError>;

    /// Protect a pending reservation from TTL expiry before an irreversible remote action.
    async fn commit(&self, _id: ReservationId) -> Result<(), LedgerStoreError> {
        Ok(())
    }

    /// Record settlement: mark the reservation `Posted` and store the per-source fills. A no-op
    /// unless the reservation is currently `Pending`, so a replay does not re-post.
    async fn post(&self, id: ReservationId, filled: &[U256]) -> Result<(), LedgerStoreError>;

    /// Record a lost auction / reverted fill: mark a pending reservation `Voided`.
    async fn void(&self, id: ReservationId) -> Result<(), LedgerStoreError>;

    /// Record a TTL expiry: mark a pending reservation `Expired`.
    async fn expire(&self, id: ReservationId) -> Result<(), LedgerStoreError>;

    /// Record a reorg reversal: mark a posted reservation `ReorgOpen`.
    async fn void_reorg(&self, id: ReservationId) -> Result<(), LedgerStoreError>;

    /// The reservations still holding capacity (`Pending`), in id order — replayed through the pure
    /// engine on restart to rebuild the in-memory holds.
    async fn open_reservations(&self) -> Result<Vec<Reservation>, LedgerStoreError>;
}

/// A ledger store failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LedgerStoreError {
    /// The id exists with a terminal state or different immutable reservation body.
    #[error("reservation {0} conflicts with the stored reservation")]
    Conflict(ReservationId),
    /// The database call or a payload (de)serialization failed.
    #[error("db: {0}")]
    Db(String),
}
