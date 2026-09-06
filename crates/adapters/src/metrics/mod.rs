//! Maker-analytics adapters: capture (quote log) and, later, the metric rollups.

pub mod quote_log;

pub use quote_log::SqliteQuoteLog;
