//! Ingest value types: the canonical protocol-agnostic intent and its amount curve — the contract
//! every downstream slice (routing, ledger, execution) consumes.

pub mod curve;
pub mod fee;
pub mod intent;
pub mod raw;

pub use curve::{AmountCurve, Rounding};
pub use fee::ExecutionFeePolicy;
pub use intent::{Exclusivity, Intent, IntentInput, IntentOutput, ProtocolId};
pub use raw::RawOrder;
