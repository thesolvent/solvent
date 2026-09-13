//! Asset value types: the token-list entries we ingest and the complete `Asset` we serve. One
//! `Asset` carries everything about a token — identity, static metadata, market, protocol status —
//! so callers never stitch it together from several places.

use alloy_primitives::Address;
use serde::{Deserialize, Serialize};

/// One entry in a token list (Uniswap Token List shape — camelCase, so real lists ingest directly).
#[derive(Debug, Clone, Deserialize)]
pub struct TokenMeta {
    #[serde(rename = "chainId")]
    pub chain_id: u64,
    pub address: Address,
    pub symbol: String,
    pub name: String,
    pub decimals: u8,
    #[serde(rename = "logoURI", default)]
    pub logo_uri: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A token list — the static source of asset metadata.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenList {
    pub name: String,
    pub tokens: Vec<TokenMeta>,
}

/// A token's identity + display essentials — embedded wherever a response references a token
/// without the full [`Asset`] (pool base/quote, trade flows, roster).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Token {
    #[schema(value_type = String)]
    pub address: Address,
    pub chain_id: u64,
    pub symbol: String,
    pub logo_uri: Option<String>,
    pub decimals: u8,
}

/// The complete picture of one asset: identity + static metadata + market + protocol status. Every
/// asset query returns this, and the API serializes it verbatim.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Asset {
    #[schema(value_type = String)]
    pub address: Address,
    pub chain_id: u64,
    pub symbol: String,
    pub name: String,
    pub decimals: u8,
    pub tags: Vec<String>,
    pub logo_uri: Option<String>,
    /// USD price and its 24h change — `None` until the price feed is wired.
    pub price_usd: Option<f64>,
    pub change_24h_pct: Option<f64>,
    /// Whether ≥1 active strategy quotes this asset, how many, and the pairs it trades in.
    pub supported: bool,
    pub active_strategy_count: u64,
    pub pairs: Vec<String>,
}

/// One tradeable pair offered by the Create wizard: the two tokens, their kind, mid price, the
/// suggested defaults, and (when a wallet is supplied) the maker's balance of each side.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PairInfo {
    pub base: Token,
    pub quote: Token,
    /// `stable` when both sides are stablecoins, else `volatile`.
    #[serde(rename = "type")]
    pub kind: PairKind,
    /// Mid price, quote per 1 base — `None` when either side is unpriced.
    pub mid: Option<f64>,
    pub default_fee_bps: u32,
    pub default_band_pct: f64,
    /// The maker's wallet balance of each side; present only when a wallet was supplied.
    pub wallet: Option<PairWallet>,
}

/// Whether a pair is stable/stable or involves a volatile asset — drives the wizard's default curve.
#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PairKind {
    Stable,
    Volatile,
}

/// A maker's wallet balance of each side of a pair, as display numbers and exact base units.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PairWallet {
    pub base: f64,
    pub quote: f64,
    pub base_raw: String,
    pub quote_raw: String,
}

/// The history window requested by a price chart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize, utoipa::ToSchema)]
pub enum PriceHistoryPeriod {
    #[serde(rename = "7d")]
    SevenDays,
    #[serde(rename = "3m")]
    ThreeMonths,
    #[serde(rename = "all")]
    All,
}

/// One timestamped pair midpoint, quoted as `quote` per one `base`.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct PairPricePoint {
    pub timestamp_ms: u64,
    pub price: f64,
    /// USD-denominated market volume for the interval when the source provides it.
    pub volume_usd: Option<f64>,
}

/// Historical market data for one oriented pair.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct PairPriceHistory {
    #[schema(value_type = String)]
    pub base: Address,
    #[schema(value_type = String)]
    pub quote: Address,
    pub period: PriceHistoryPeriod,
    pub points: Vec<PairPricePoint>,
}
