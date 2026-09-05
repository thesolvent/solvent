//! Pool value types — the aggregation of active strategies over one pair, served directly.

use serde::Serialize;

use super::asset::Token;

/// How a pool's makers price the pair — drives filtering and display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub enum PoolType {
    Stable,
    Correlated,
    Volatile,
}

/// A pool: the aggregation of all active, priceable strategies over one canonical pair. `$` and
/// volume/fills fields are absent/zero until pricing (M3) and the trade store (M2) land.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Pool {
    pub pair: String,
    pub base: Token,
    pub quote: Token,
    #[serde(rename = "type")]
    pub pool_type: PoolType,
    pub maker_count: u64,
    pub min_spread_bps: u32,
    pub max_spread_bps: u32,
    pub popular_fee_tier: String,
    pub tvl_usd: Option<f64>,
    pub volume_24h_usd: Option<f64>,
    pub fills_24h: u64,
    pub apr_pct: Option<f64>,
}
