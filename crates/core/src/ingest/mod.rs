//! Ingest: fan protocol order feeds into one clean, deduped `Intent` stream for the decision loop.

pub mod service;

pub use service::IngestPipeline;
