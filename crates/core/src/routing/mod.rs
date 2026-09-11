//! Routing — reduce an intent to a bounded candidate set (this phase), then source it
//! across maker curves at the marginal-price optimum (the solver).

pub mod candidates;
pub mod guard;
pub mod leg_cost;
pub mod service;
pub mod waterfill;

pub use candidates::{price_impact_pct, select, Candidate, Selection};
#[cfg(feature = "quote-metrics")]
pub use candidates::{quote_calls, reset_quote_calls};
pub(crate) use guard::GuardAdmission;
pub use guard::{GuardSnapshot, StrategyGuard};
pub use leg_cost::LegCostResolver;
pub use service::{resolve_leg_cost, route, RoutingBook};
pub use waterfill::{solve, solve_sparse, Split};
