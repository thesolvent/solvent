//! Ledger — the single-writer service that makes the pure reservation engine durable,
//! concurrency-safe, and recoverable. It syncs every active maker's caps off the request path and
//! publishes one lock-free `available` snapshot (caps net of holds) that the depth curve and the
//! router both read.

pub mod service;

pub use service::{AvailableSnapshot, LedgerService};
