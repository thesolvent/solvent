//! The clock port: the ledger's only source of "now", so TTL expiry is testable.

/// A source of wall-clock time (infallible).
pub trait Clock: Send + Sync {
    /// Seconds since the Unix epoch.
    fn now_unix(&self) -> u64;
}
