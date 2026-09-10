//! Execution value types: the protocol-agnostic fill transaction and the lifecycle status the
//! execution service couples back to the ledger.

pub mod authorization;
pub mod fill;

pub use authorization::{
    ExecutionAuthorization, ExecutionKind, PolicySignature, RebateAuthorization,
    UserFillAuthorization,
};
pub use fill::{
    ExecHandle, ExecStatus, FillOutcome, FillTx, PendingFill, Settled, SettledOutcome, SimVerdict,
    TrackedFill,
};
