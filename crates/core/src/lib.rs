//! Solvent core — value primitives, ports, and domain logic. Zero I/O; cannot depend on adapters.

pub mod deps;
pub mod obs;
pub mod primitives;
pub mod registry;

pub use primitives::SolventError;
