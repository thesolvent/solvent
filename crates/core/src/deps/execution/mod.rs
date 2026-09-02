//! Execution ports: the tx engine that submits a fill and tracks it to a terminal status.

pub mod engine;

pub use engine::{Execution, ExecutionError};
