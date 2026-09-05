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
    /// USD price and its 24h change — `None` until the price feed is wired (M3).
    pub price_usd: Option<f64>,
    pub change_24h_pct: Option<f64>,
    /// Whether ≥1 active strategy quotes this asset, how many, and the pairs it trades in.
    pub supported: bool,
    pub active_strategy_count: u64,
    pub pairs: Vec<String>,
}
