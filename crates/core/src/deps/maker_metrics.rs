//! The maker-metrics port: read-side analytics aggregated from the quote log and the trade store.
//! Returns raw, USD-free numbers — the DTO layer applies `Valuation` for USD, fees, and APY.

use alloy_primitives::{Address, U256};
use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::registry::TokenPair;
use crate::primitives::{MakerId, StrategyHash};

/// Delivered volume for one token, in base units (valued to USD at the DTO layer).
pub struct TokenVolume {
    pub token: Address,
    pub base_units: U256,
}

/// A maker's activity over a window — the dashboard KPIs and inventory rollups.
pub struct MakerMetrics {
    /// Confirmed trades that sourced this maker, in the window.
    pub fills: u64,
    /// Fills per day for the last 7 days, oldest bucket first.
    pub fills_by_day: [u64; 7],
    pub last_fill_at: Option<u64>,
    /// Per-token delivered volume — the base for USD volume and fee figures.
    pub volume: Vec<TokenVolume>,
    /// Quotes the maker was sourced into (the fill-share denominator).
    pub quotes: u64,
    pub latency_p50_ms: Option<u64>,
}

/// One position's activity over a window — the `active_stats` block.
pub struct PositionMetrics {
    pub fills: u64,
    pub volume: Vec<TokenVolume>,
    pub last_fill_at: Option<u64>,
    /// Quotes this strategy joined ÷ quotes on its pair, in the window.
    pub quote_uptime_pct: Option<f64>,
}

#[async_trait]
pub trait MakerMetricsStore: Send + Sync {
    /// Maker-scoped metrics over `[since, now]` (`now` sizes the 7-day fills sparkline).
    async fn maker(
        &self,
        maker: MakerId,
        since: u64,
        now: u64,
    ) -> Result<MakerMetrics, MakerMetricsError>;

    /// Position-scoped metrics for `strategy` on `pair`, since `since`.
    async fn position(
        &self,
        strategy: StrategyHash,
        pair: TokenPair,
        since: u64,
    ) -> Result<PositionMetrics, MakerMetricsError>;
}

/// A maker-metrics failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MakerMetricsError {
    /// The database call or a payload conversion failed.
    #[error("db: {0}")]
    Db(String),
}
