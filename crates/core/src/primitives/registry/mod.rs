//! Registry value types: the Aqua event model and the derived liquidity snapshot.

pub mod curve;
pub mod event;
pub mod range;
pub mod snapshot;
pub mod strategy;

pub use curve::{curve_label, decode_strategy, Curve, CurveSpec, PeggedParams};
pub use event::{AquaEvent, EventCursor, EventExt, StrategyKey};
pub use range::{price_range, PositionRange, RangeKind};
pub use snapshot::{fee_in_bps, ActiveAsset, CurveKind, PoolStats, Snapshot, StrategyCount};
pub use strategy::{MakerStrategy, TokenPair};
