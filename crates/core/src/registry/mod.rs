//! Registry — a live, event-sourced picture of maker Aqua liquidity that prices
//! the three Aqua strategy types closed-form.

pub mod curves;
pub mod pricing;
pub mod service;

pub use curves::{ConcentratePool, CurveError, PeggedPool, Pricing, XycPool};
pub use pricing::{price, PriceError};
pub use service::SharedSnapshot;
