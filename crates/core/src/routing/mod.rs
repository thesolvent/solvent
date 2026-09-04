//! Routing — reduce an intent to a bounded candidate set (this phase), then source it
//! across maker curves at the marginal-price optimum (the solver).

pub mod candidates;

pub use candidates::{select, Candidate};
