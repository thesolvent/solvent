//! Routing — reduce an intent to a bounded candidate set (this phase), then source it
//! across maker curves at the marginal-price optimum (the solver).

pub mod candidates;
pub mod service;
pub mod waterfill;

pub use candidates::{select, Candidate, Selection};
pub use service::{resolve_leg_cost, route};
pub use waterfill::{solve, solve_sparse, Solution};
