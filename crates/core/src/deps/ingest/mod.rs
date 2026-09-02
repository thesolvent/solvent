//! Ingest ports: turning raw feed orders into canonical intents.

pub mod normalizer;

pub use normalizer::{NormalizeError, Normalizer};
