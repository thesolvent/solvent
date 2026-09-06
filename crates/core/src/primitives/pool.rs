//! Pool value types — the aggregation of active strategies over one pair, served directly.

use alloy_primitives::Address;
use serde::{Deserialize, Serialize};

use super::amount::TokenAmounts;
use super::asset::Token;

/// How a pool's makers price the pair — drives filtering and display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub enum PoolType {
    Stable,
    Correlated,
    Volatile,
}

/// A pool: the aggregation of all active, priceable strategies over one canonical pair. USD and
/// volume/fills fields are absent/zero until their data sources (pricing, the trade store) exist.
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

/// One maker's strategy in a pool: curve, fee, and committed (`virtual`) balances. Actual/pullable
/// (on-chain) balances and uptime are absent until the balance reader and metrics capture exist.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PoolMaker {
    #[schema(value_type = String)]
    pub maker: Address,
    pub strategy_hash: String,
    pub curve: String,
    pub fee_bps: u32,
    #[serde(rename = "virtual")]
    pub virtual_balances: TokenAmounts,
}

/// Pool detail: the list row plus the maker roster. The roster is the only field the detail view
/// adds over [`Pool`], so it composes one rather than restating every KPI. Depth/price-impact and
/// recent fills join as their sources land.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PoolDetail {
    #[serde(flatten)]
    pub pool: Pool,
    pub makers: Vec<PoolMaker>,
}

/// The trade direction a depth curve is plotted for; the pair's curves are asymmetric, so buying the
/// base and selling it hit different inventory and price differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Buy,
    Sell,
}

/// The pool's combined executable liquidity as a curve: for each sampled trade size, the output, its
/// blended price, and how far that price sits below the tip. Reconstructs an order book the pool
/// doesn't have by stacking every active maker's curve (the router's split, plotted across sizes).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PoolDepth {
    pub axis_title: String,
    pub best_price: String,
    pub points: Vec<DepthPoint>,
}

/// One point on the depth curve, anchored to a price-impact bucket. Amounts are base-unit strings;
/// `impact_pct` is how far `effective_price` sits below `best_price`, in percent.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DepthPoint {
    pub trade_size: String,
    pub output: String,
    pub effective_price: String,
    pub impact_pct: f64,
    pub makers_used: u64,
}
