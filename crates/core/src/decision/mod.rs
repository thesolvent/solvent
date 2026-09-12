//! The decision loop: when a fed order becomes worth filling, and the single attempt at it.

pub mod service;

pub use service::{DecisionConfig, DecisionDeps, DecisionService};
