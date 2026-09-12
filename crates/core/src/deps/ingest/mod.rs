//! Ingest ports: order feeds, per-protocol normalizers, and per-protocol fill builders.

pub mod feed;
pub mod fill;
pub mod normalizer;

pub use feed::OrderFeed;
pub use fill::{FillBuilder, FillBuilderError, PreparedFill};
pub use normalizer::{NormalizeError, Normalizer};
