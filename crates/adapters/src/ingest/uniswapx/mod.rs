//! UniswapX V2 ingest adapters.

mod builder;
mod codec;
mod feed;
mod normalizer;

pub use builder::{OrderSpec, SignedOrderBuilder};
pub use feed::SelfHostedFeed;
pub use normalizer::UniswapXV2Normalizer;
