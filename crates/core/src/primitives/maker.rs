//! Maker-facing read types: a position (one shipped strategy, seen as liquidity) and the roster
//! entry. `Position` is the canonical resource; the positions list is a projection that omits the
//! detail-only `active_stats` and `opening` balances.

use alloy_primitives::Address;
use serde::Serialize;

use super::amount::{Amount, TokenAmounts};
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
    /// Current curve spot in `quote per base` — filled by the marginal-price task.
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

/// A position's committed vs. pullable balances. `opening` (the ship-time deposit) is detail-only, so
/// it is empty in the list projection.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PositionBalances {
    #[serde(rename = "virtual")]
    pub virtual_balances: TokenAmounts,
    pub actual: TokenAmounts,
    /// Whether every committed token is fully pullable on-chain (no `shortfall`).
    pub backed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortfall: Option<String>,
    /// Pullable USD as a fraction of committed USD (`1.0` = fully backed).
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

/// A maker's dashboard: headline KPIs (with period-over-period deltas), fill-share on the pairs it
/// quotes, fill latency, and a competitive insight. USD values are `None` when unpriced.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MakerDashboard {
    #[schema(value_type = String)]
    pub maker: Address,
    /// The window (days) the KPIs and deltas cover.
    pub window_days: u32,
    pub active_positions: u64,
    pub kpis: MakerKpis,
    pub fill_share: FillShare,
    pub latency_p50_ms: Option<u64>,
    /// A cheaper competitor on one of the maker's pairs, if any undercuts it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insight: Option<String>,
}

/// The dashboard KPI tiles. `*_change_pct` are period-over-period deltas (previous equal window).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MakerKpis {
    pub shared_liquidity_usd: Option<f64>,
    pub volume_usd: Option<f64>,
    pub wallet_balance_usd: Option<f64>,
    pub pullable_usd: Option<f64>,
    /// Pullable ÷ shared liquidity — aggregate backing coverage, rendered "×".
    pub shared_liq_ratio: Option<f64>,
    pub fees_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_liquidity_change_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_change_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fees_change_pct: Option<f64>,
}

/// The maker's fills as a share of all fills on the pairs it quotes.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct FillShare {
    /// The maker's confirmed fills in the window.
    pub filled: u64,
    /// All confirmed fills on the maker's pairs in the window (the share denominator).
    pub pair_fills: u64,
    pub share_pct: Option<f64>,
}

/// One token in a maker's inventory: its wallet vs committed balances, aggregate economics, and the
/// positions (`legs`) that hold it. The maker's positions re-grouped by token (the Assets tab).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct InventoryRow {
    pub token: Token,
    /// The maker's wallet balance of this token (uncapped).
    pub wallet: Amount,
    /// Committed across all the maker's positions.
    pub shared: Amount,
    /// Fees from every position holding this token (a position counts under each token it holds).
    pub fees_usd: Option<f64>,
    pub apy_pct: Option<f64>,
    pub legs: Vec<InventoryLeg>,
}

/// One position's slice of a token: this token's committed vs opening balance in that position, plus
/// the position's economics (repeated per leg).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct InventoryLeg {
    pub pair: String,
    pub curve: String,
    pub fee_bps: u32,
    /// This token's committed balance in the position.
    pub current: Amount,
    /// This token's ship-time balance in the position.
    pub opening: Amount,
    pub volume_usd: Option<f64>,
    pub fees_usd: Option<f64>,
    pub apy_pct: Option<f64>,
    pub coverage: Option<f64>,
}

/// Server-authoritative pre-flight for a ship the SDK already encoded: whether the strategy
/// already exists, which tokens still need an approval, and any warnings.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PreviewResponse {
    /// A strategy with this hash already exists — a re-ship would collide.
    pub exists: bool,
    /// Tokens whose allowance to Aqua is below the amount to ship; approve them first.
    #[schema(value_type = Vec<String>)]
    pub requires_approval: Vec<Address>,
    /// Human warnings, e.g. insufficient balance for a leg.
    pub warnings: Vec<String>,
}
