//! The quote slice: the read-only routing estimate served at `POST /swap/quote`.

mod service;

pub use crate::primitives::quote::{QuoteLeg, QuoteResponse};
pub use service::QuoteService;
