//! Price-restoration policy and its in-memory orchestration.

mod policy;
mod service;
mod worker;

pub use policy::{RebateError, RebatePolicy, RebatePolicyConfig};
pub use service::{RebateEvaluationInput, RebateMarketData, RebateService, RebateServiceConfig};
pub use worker::{RebateWorker, RebateWorkerConfig, RebateWorkerReport};
