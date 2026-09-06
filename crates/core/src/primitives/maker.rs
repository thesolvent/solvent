//! Maker-facing read types: a position (one shipped strategy, seen as liquidity) and the roster
//! entry. `Position` is the canonical resource; the positions list is a projection that omits the
//! detail-only `active_stats` (and, later, the band chart and fills).

use alloy_primitives::Address;
use serde::Serialize;

use super::amount::TokenAmounts;
use super::asset::Token;
use super::pool::PoolType;

/// A maker's position — one shipped strategy, valued and analyzed.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Position {
    pub strategy_hash: String,
    pub pair: String,
    pub base: Token,
    pub quote: Token,
    #[schema(value_type = String)]
    pub maker: Address,
    pub curve: String,
    pub pair_type: PoolType,
    pub state: String,
    pub fee_bps: u32,
    /// First-`Shipped` / `Docked` block, and the current spot — the on-chain history, filled later.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_block: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docked_block: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mid_price: Option<String>,
    pub range: PriceRange,
    pub balances: PositionBalances,
    pub economics: Economics,
    /// Detail-only (route 16): omitted from the list projection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_stats: Option<ActiveStats>,
}

/// A position's price range in human `quote per base` terms; which fields are set depends on `kind`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PriceRange {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lower_price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upper_price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peg_price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub below_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub above_pct: Option<f64>,
    pub label: String,
}

/// A position's committed vs. pullable balances. `opening`/`coverage` (from the ship history) fill in
/// later.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PositionBalances {
    #[serde(rename = "virtual")]
    pub virtual_balances: TokenAmounts,
    pub actual: TokenAmounts,
    pub backed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortfall: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<f64>,
    pub opening: TokenAmounts,
    pub split: Vec<Split>,
}

/// One token's share of a position's committed liquidity.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Split {
    pub token: Token,
    pub pct: f64,
}

/// USD economics for a position over the dashboard's window.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Economics {
    pub fees_usd: Option<f64>,
    pub apy_pct: Option<f64>,
    pub volume_usd: Option<f64>,
}

/// Recent-activity stats for the position detail.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ActiveStats {
    pub fills_7d: u64,
    pub volume_7d_usd: Option<f64>,
    pub quote_uptime_pct: Option<f64>,
    pub last_fill_at: Option<u64>,
}

/// One maker in the roster.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MakerSummary {
    #[schema(value_type = String)]
    pub maker: Address,
    pub active_positions: u64,
    pub shared_liquidity_usd: Option<f64>,
}
