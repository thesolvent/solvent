//! The settlement-reader port: read the *actual* per-source amounts a confirmed fill pulled, so the
//! ledger posts what really happened (not the conservative hold). One impl per protocol — the live
//! reader decodes the fill tx's own settlement events; a canonical-watcher-sourced reader can slot
//! in behind the same port later.

use alloy_primitives::{B256, U256};
use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::ledger::ReservationSource;

/// Reads the actual amounts a confirmed fill pulled per source. The returned vector is aligned
/// index-for-index with `sources` — `settled(...)[i]` is what was pulled for `sources[i]`, and a
/// source the fill never touched is `0`. Alignment lives here because matching on-chain events to
/// reservation sources is protocol-specific.
#[async_trait]
pub trait SettlementReader: Send + Sync {
    async fn settled(
        &self,
        tx: B256,
        sources: &[ReservationSource],
    ) -> Result<Vec<U256>, SettlementError>;
}

/// A failure reading the settlement (the receipt was unreachable or undecodable).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SettlementError {
    #[error("settlement read: {0}")]
    Read(String),
}
