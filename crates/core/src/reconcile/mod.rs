//! The reconcile slice: drive in-flight fills to a terminal state and settle their trade lifecycle,
//! plus reclaim orphaned reservation holds. Runs off-request on a cadence.

mod service;

pub use service::{ReconcileReport, ReconcileService};
