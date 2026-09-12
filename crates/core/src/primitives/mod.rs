//! Value primitives, organized per domain: `shared` is the cross-cutting foundation; domain-specific
//! modules (`registry`, `ledger`, …) sit alongside it.

pub mod amount;
pub mod asset;
pub mod crosschain;
pub mod execution;
pub mod ingest;
pub mod ledger;
pub mod maker;
pub mod pool;
pub mod pricing;
pub mod quote;
pub mod rebate;
pub mod registry;
pub mod routing;
pub mod shared;
pub mod trade;

pub use shared::{
    AggregateQuoteId, Bps, ChainConfig, ChainId, CrossChainOrderId, CrossChainStepId, FillId,
    IntentId, LeaseId, MakerId, PrepareToken, RebateBatchId, ReservationId, SolventError,
    StrategyHash, SystemConfig, Usd, UsdPrice,
};
