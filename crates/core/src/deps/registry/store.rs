//! The registry store port: a durable per-chain event log plus a resume cursor.
//! The event log is the source of truth the snapshot is rebuilt from on restart;
//! the cursor records how far the watcher has scanned.

use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::registry::{AquaEvent, EventCursor, EventExt};
use crate::primitives::ChainId;

#[async_trait]
pub trait Store: Send + Sync {
    /// How far `chain` has been scanned, or `None` before the first cycle.
    async fn cursor(&self, chain: ChainId) -> Result<Option<EventCursor>, StoreError>;

    /// Append `events`, returning the ones that were **newly** inserted. The
    /// unique key deduplicates overlap re-scans and late re-deliveries, so only
    /// the returned events are folded into the snapshot.
    async fn insert(
        &self,
        chain: ChainId,
        events: &[EventExt<AquaEvent>],
    ) -> Result<Vec<EventExt<AquaEvent>>, StoreError>;

    /// Record how far `chain` has been scanned. Written last in a cycle — after
    /// the events are durable and the snapshot reflects them — so a crash mid-cycle
    /// simply re-scans rather than skipping unprocessed blocks.
    async fn save_cursor(&self, chain: ChainId, cursor: EventCursor) -> Result<(), StoreError>;

    /// The full event log for `chain` in fold order — replayed to rebuild the
    /// snapshot on startup.
    async fn events(&self, chain: ChainId) -> Result<Vec<EventExt<AquaEvent>>, StoreError>;
}

/// A store failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StoreError {
    /// The database call failed.
    #[error("db: {0}")]
    Db(String),
    /// A log arrived without a mined `(block_number, log_index)` position, so it
    /// cannot be keyed. The chain source only yields mined logs, so this marks a
    /// contract-violating adapter rather than a normal condition.
    #[error("event has no mined position")]
    Unpositioned,
}
