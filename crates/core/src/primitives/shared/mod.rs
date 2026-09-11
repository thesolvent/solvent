//! Cross-cutting value types used by every phase: errors, token/USD/bps valuations, typed IDs,
//! and read-only config.

pub mod config;
pub mod error;
pub mod ids;
pub mod valuation;

pub use config::{ChainConfig, SystemConfig};
pub use error::SolventError;
pub use ids::{
    AggregateQuoteId, ChainId, CrossChainOrderId, CrossChainStepId, FillId, IntentId, LeaseId,
    MakerId, PrepareToken, RebateBatchId, ReservationId, StrategyHash,
};
pub use valuation::{Bps, Usd, UsdPrice};
