//! A cached chain head. A poller future refreshes the latest block number on a fixed cadence so
//! request handlers read it lock-free — no RPC, no `await`, no failure — instead of hitting the node
//! on every request. The adapter returns the poller; the app spawns it (the app owns the runtime).

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use alloy::providers::{DynProvider, Provider};

/// The latest observed block number, kept fresh by a poller. [`latest`](Self::latest) is a single
/// atomic read, safe to call on every request.
#[derive(Clone)]
pub struct ChainHead {
    latest: Arc<AtomicU64>,
}

impl ChainHead {
    /// A cached head and the poller that keeps it fresh. The caller spawns `poller` on its runtime;
    /// `latest()` then reflects each observed block number. A failed poll is logged and the previous
    /// value is kept.
    pub fn new(provider: DynProvider, interval: Duration) -> (Self, impl Future<Output = ()>) {
        let latest = Arc::new(AtomicU64::new(0));
        let shared = Arc::clone(&latest);
        let poller = async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                match provider.get_block_number().await {
                    Ok(block) => shared.store(block, Ordering::Relaxed),
                    Err(e) => tracing::warn!(error = %e, "block-number poll failed"),
                }
            }
        };
        (Self { latest }, poller)
    }

    /// The most recently observed block number (`0` until the first successful poll).
    pub fn latest(&self) -> u64 {
        self.latest.load(Ordering::Relaxed)
    }

    /// A fixed head for tests — no poller.
    #[cfg(test)]
    pub(crate) fn stub(block: u64) -> Self {
        Self {
            latest: Arc::new(AtomicU64::new(block)),
        }
    }
}
