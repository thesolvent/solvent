//! HTTP route handlers — thin translators from a request to a core call to a response envelope.
//! Business logic lives in core services, never here.

pub mod assets;
pub mod config;
pub mod pools;
pub mod stats;
