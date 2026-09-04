//! Ledger — the single-writer service that makes the pure reservation engine durable,
//! concurrency-safe, and recoverable.

pub mod service;

pub use service::{AvailableSnapshot, LedgerService};
