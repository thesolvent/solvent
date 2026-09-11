//! 1inch Limit Order Protocol v4 ingest: feed, normalizer, and a `FillBuilder` for
//! `OneInchLimitOrderAquaFiller` — the on-chain filler that sources a fill's taker-asset leg from
//! Aqua/SwapVM inside 1inch's own taker-interaction callback.

mod codec;
mod feed;
mod fill;
mod normalizer;
mod orders_api;

pub use feed::OneInchFeed;
pub use fill::OneInchFillBuilder;
pub use normalizer::OneInchNormalizer;
pub use orders_api::{OneInchApiClient, OneInchApiError, OrderRecord};
