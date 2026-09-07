//! The quote-log port: record each quote served so uptime, latency, and fill-share can be
//! aggregated. Best-effort telemetry — a failure here never fails the quote.

use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::registry::TokenPair;
use crate::primitives::{ChainId, MakerId, StrategyHash};

/// One strategy sourced into a served quote.
pub struct QuoteParticipant {
    pub maker: MakerId,
    pub strategy_hash: StrategyHash,
}

/// A quote served for a pair: how long routing took and which strategies it sourced (empty when no
/// route existed). The id and `served_at` are stamped by the adapter.
pub struct QuoteServed {
    pub chain_id: ChainId,
    pub pair: TokenPair,
    pub latency_ms: u64,
    pub participants: Vec<QuoteParticipant>,
}

#[async_trait]
pub trait QuoteLog: Send + Sync {
    /// Persist one served quote. Best-effort — callers log and continue on error.
    async fn record(&self, quote: &QuoteServed) -> Result<(), QuoteLogError>;
}

/// A quote-log failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum QuoteLogError {
    /// The database call failed.
    #[error("db: {0}")]
    Db(String),
}
