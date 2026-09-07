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
    /// Per-token delivered volume (outflow) — the base for USD volume and fee figures.
    pub volume: Vec<TokenVolume>,
    /// Per-token received amount (inflow) — with `volume`, the net liquidity flow from trading.
    pub inflow: Vec<TokenVolume>,
    /// Quotes the maker was sourced into.
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

/// A pair's trading activity over a window — the pool row's volume, fills and yield.
pub struct PairMetrics {
    /// Confirmed trades settled on the pair, in the window.
    pub fills: u64,
    /// Per-token delivered volume (outflow) across those trades.
    pub volume: Vec<TokenVolume>,
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

    /// How many confirmed trades settled on any of `pairs` since `since` — the fill-share
    /// denominator (the maker's share of the fills on the pairs it quotes).
    async fn pair_fills(&self, pairs: &[TokenPair], since: u64) -> Result<u64, MakerMetricsError>;

    /// Fills and delivered volume on one pair since `since`, whichever maker served them.
    async fn pair_activity(
        &self,
        pair: TokenPair,
        since: u64,
    ) -> Result<PairMetrics, MakerMetricsError>;
}

/// A maker-metrics failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MakerMetricsError {
    /// The database call or a payload conversion failed.
    #[error("db: {0}")]
    Db(String),
}
