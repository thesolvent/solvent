//! Value primitives, organized per domain: `shared` is the cross-cutting foundation; domain-specific
//! modules (`registry`, `ledger`, …) sit alongside it.

pub mod shared;

pub use shared::{
    Bps, FillId, IntentId, LeaseId, MakerId, ReservationId, SolventError, StrategyHash,
    SystemConfig, Usd, UsdPrice,
};
