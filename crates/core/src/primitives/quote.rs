//! The quote response — a routed, read-only estimate: the split across makers, the blended output,
//! and its price impact. Built by the quote service from a `RoutePlan` and serialized straight to
//! the wire (like the pool views).

use alloy_primitives::Address;
use serde::Serialize;
use utoipa::ToSchema;

use crate::asset::Token;
use crate::primitives::amount::Amount;

/// One maker's slice of a quoted route.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct QuoteLeg {
    #[schema(value_type = String)]
    pub maker: Address,
    pub strategy_hash: String,
    pub token_in: Token,
    pub token_out: Token,
    pub amount_in: Amount,
    pub amount_out: Amount,
    /// The curve shape: `XYC` | `Concentrated` | `Pegged`.
    pub curve: String,
    /// This leg's share of the blended output, in percent.
    pub share_pct: f64,
}

/// A read-only quote: the routed split, its blended output, and price impact. `quote_id` is
/// deterministic from the request; `expires_at` is an advisory RFC-3339 instant (a quote does not
/// bind a future swap).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct QuoteResponse {
    pub quote_id: String,
    pub amount_out: Amount,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executor_fee: Option<Amount>,
    pub price_impact_pct: f64,
    pub makers_sourced: u32,
    pub legs: Vec<QuoteLeg>,
    pub expires_at: String,
}
