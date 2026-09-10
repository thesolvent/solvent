//! Price-restoration policy and its in-memory orchestration.

mod policy;
mod service;

pub use policy::{RebateError, RebatePolicy, RebatePolicyConfig};
pub use service::{RebateEvaluationInput, RebateMarketData, RebateService, RebateServiceConfig};
