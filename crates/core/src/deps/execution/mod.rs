//! Execution ports: the tx engine that submits a fill and tracks it to a terminal status, and the
//! durable store that keeps the in-flight set across a restart.

pub mod authorizer;
pub mod engine;
pub mod settlement;
pub mod sim;
pub mod store;

pub use authorizer::{ExecutionAuthorizer, ExecutionAuthorizerError};
pub use engine::{Execution, ExecutionError};
pub use settlement::{SettlementError, SettlementReader};
pub use sim::{SimError, SimGate};
pub use store::{FillStore, FillStoreError};
