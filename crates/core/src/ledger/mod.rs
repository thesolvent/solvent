//! Ledger — the single-writer service that makes the pure reservation engine durable,
//! concurrency-safe, and recoverable, plus the budget cache that syncs every maker's caps for the
//! lock-free read paths.

pub mod budget_cache;
pub mod service;

pub use budget_cache::BudgetCache;
pub use service::{AvailableSnapshot, LedgerService};
