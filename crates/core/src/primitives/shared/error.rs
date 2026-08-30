//! The one public error type. Each port defines its own `{Trait}Error` that maps in via `From`;
//! a variant is added only when a consumer needs it — and no classification (`kind()`) until a
//! caller actually branches on it.

use thiserror::Error;

use crate::deps::registry::{ChainSourceError, StoreError};

/// The error every fallible Solvent API returns.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SolventError {
    /// A typed identifier could not be parsed from its input.
    #[error("invalid {id_type} id: {reason}")]
    InvalidId {
        id_type: &'static str,
        reason: String,
    },
    /// Reading Aqua events from the chain failed.
    #[error("chain source: {0}")]
    ChainSource(#[from] ChainSourceError),
    /// The registry store failed.
    #[error("store: {0}")]
    Store(#[from] StoreError),
}
