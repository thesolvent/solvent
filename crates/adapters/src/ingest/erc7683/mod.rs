//! ERC-7683 (v1, same-chain) ingest adapters.

mod builder;
mod codec;
mod feed;
mod normalizer;

pub use builder::{OrderSpec, SignedOrderBuilder};
pub use feed::SelfHostedFeed;
pub use normalizer::Erc7683Normalizer;
