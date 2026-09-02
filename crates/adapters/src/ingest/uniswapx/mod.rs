//! UniswapX V2 ingest adapters.

mod builder;
mod codec;
mod feed;
mod fill;
mod normalizer;

pub use builder::{OrderSpec, SignedOrderBuilder};
pub use feed::SelfHostedFeed;
pub use fill::UniswapXFillBuilder;
pub use normalizer::UniswapXV2Normalizer;
