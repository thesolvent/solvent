//! The vendored garden-rs log indexer: the `event_provider!` macro, the typed
//! `IndexerError`, and the alloy-`Log` → `EventExt` bridge.

mod indexer;

pub use indexer::*;

use alloy::rpc::types::Log;
use solvent_core::primitives::registry::EventExt;

/// Wrap a decoded `event` with the provenance of the `log` it came from. The
/// type lives in `solvent-core`; the orphan rule bars a `From<Log>` impl here
/// (all three foreign), so the conversion is a free fn. Adapted from garden-rs.
pub fn event_ext<T>(log: &Log, event: T) -> EventExt<T> {
    EventExt {
        event,
        address: log.inner.address,
        block_hash: log.block_hash,
        block_number: log.block_number,
        transaction_hash: log.transaction_hash,
        transaction_index: log.transaction_index,
        log_index: log.log_index,
        removed: log.removed,
    }
}
