//! Ports: one trait per file, each behind `Arc<dyn Trait>` with its own `{Trait}Error`, defined
//! with the component that owns it.

pub mod balances;
pub mod execution;
pub mod ingest;
pub mod ledger;
pub mod maker_metrics;
pub mod quote_log;
pub mod registry;
pub mod routing;
pub mod trade;
