//! The recapture slice: on each confirmed fill, value its legs against the oracle mid, split the
//! recaptured LVR back to the makers it rebalanced, and accrue the credits; the payout service then
//! rebates the outstanding credits to makers.

pub mod payout;
pub mod service;

pub use payout::PayoutService;
pub use service::RecaptureService;
