//! Value primitives, organized per domain: `shared` is the cross-cutting foundation; domain-specific
//! modules (`registry`, `ledger`, …) sit alongside it.

pub mod amount;
pub mod asset;
pub mod execution;
pub mod ingest;
pub mod ledger;
pub mod pool;
pub mod pricing;
pub mod registry;
pub mod routing;
pub mod shared;

pub use shared::{
    Bps, ChainConfig, ChainId, FillId, IntentId, LeaseId, MakerId, ReservationId, SolventError,
    StrategyHash, SystemConfig, Usd, UsdPrice,
};
