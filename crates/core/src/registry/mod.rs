//! Registry — a live, event-sourced picture of maker Aqua liquidity that prices
//! the three Aqua Tier-0 strategy types closed-form (design §7).

pub mod curves;

pub use curves::{ConcentratePool, CurveError, PeggedParams, PeggedPool, Pricing, XycPool};
