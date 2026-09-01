//! Ports: one trait per file, each behind `Arc<dyn Trait>` with its own `{Trait}Error`, defined
//! with the component that owns it.

pub mod ledger;
pub mod registry;
pub mod routing;
