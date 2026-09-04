//! Registry — a live, event-sourced picture of maker Aqua liquidity that prices
//! the three Aqua strategy types closed-form.

pub mod curves;
pub mod pricing;
pub mod service;
pub mod sync;

pub use curves::{
    apply_flat_fee_in, apply_flat_fee_out, ConcentratePool, CurveError, CurvePool, PeggedPool,
    Pricing, XycPool,
};
pub use pricing::{price, PriceError};
pub use service::SharedSnapshot;
pub use sync::RegistrySync;
