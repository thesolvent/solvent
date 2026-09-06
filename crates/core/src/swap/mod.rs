//! The swap slice: the write path served at `POST /swap` — route, reserve, persist, and fill one
//! taker-signed intent.

mod service;

pub use service::{SwapConfig, SwapOutcome, SwapService};
