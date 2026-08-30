//! The shared snapshot handle: one writer publishes a fresh snapshot, many
//! readers load it lock-free on the quote hot path.

use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::primitives::registry::Snapshot;

/// Lock-free single-writer / multi-reader holder for the registry snapshot.
#[derive(Default)]
pub struct SharedSnapshot(ArcSwap<Snapshot>);

impl SharedSnapshot {
    pub fn new(snapshot: Snapshot) -> Self {
        Self(ArcSwap::from_pointee(snapshot))
    }

    /// A consistent, cheap read handle valid for as long as it's held.
    pub fn load(&self) -> Arc<Snapshot> {
        self.0.load_full()
    }

    /// Publish a new snapshot; readers holding an older handle keep seeing it.
    pub fn store(&self, snapshot: Snapshot) {
        self.0.store(Arc::new(snapshot));
    }
}
