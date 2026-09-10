//! 1inch Limit Order Protocol v4 ingest — feed and normalizer only. Observation-only: there is no
//! `FillBuilder` for this protocol yet, so a 1inch `Intent` is seen, admitted or dropped, and
//! quote-priced like any other, but never submitted as a trade.

mod codec;
mod feed;
mod normalizer;
mod orders_api;

pub use feed::OneInchFeed;
pub use normalizer::OneInchNormalizer;
pub use orders_api::{OneInchApiClient, OneInchApiError, OrderRecord};
