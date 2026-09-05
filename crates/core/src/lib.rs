//! Solvent core — value primitives, ports, and domain logic. Zero I/O; cannot depend on adapters.

pub mod deps;
pub mod ingest;
pub mod ledger;
pub mod obs;
pub mod primitives;
pub mod registry;
pub mod routing;

pub use primitives::SolventError;
