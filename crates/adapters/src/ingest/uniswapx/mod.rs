//! UniswapX V2 ingest adapters.

mod builder;
mod codec;
mod normalizer;

pub use builder::{OrderSpec, SignedOrderBuilder};
pub use normalizer::UniswapXV2Normalizer;
