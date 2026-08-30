//! The clock port: the only source of "now" the ledger uses, so TTL expiry is testable with a
//! controllable clock instead of the wall clock.

/// A source of wall-clock time. Infallible — reading a clock cannot fail — so there is no
/// `{Trait}Error` here.
pub trait Clock: Send + Sync {
    /// Seconds since the Unix epoch.
    fn now_unix(&self) -> u64;
}
