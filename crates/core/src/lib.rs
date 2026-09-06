//! Solvent core — value primitives, ports, and domain logic. Zero I/O; cannot depend on adapters.

pub mod asset;
pub mod balances;
pub mod deps;
pub mod execution;
pub mod ingest;
pub mod ledger;
pub mod obs;
pub mod pool;
pub mod primitives;
pub mod quote;
pub mod registry;
pub mod routing;

pub use primitives::SolventError;
