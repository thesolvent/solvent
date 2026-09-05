//! Ingest ports: order feeds and per-protocol normalizers.

pub mod feed;
pub mod normalizer;

pub use feed::OrderFeed;
pub use normalizer::{NormalizeError, Normalizer};
