//! The trade lifecycle: a submitted swap, the makers it sourced, and the stages it passed through.
//! The public resource is a *trade*; the signed order it carries lives as fields on it, not as a
//! second type.

use core::fmt;
use core::str::FromStr;

use alloy_primitives::{Address, Bytes, B256, U256};
use serde::Serialize;

use crate::primitives::ingest::OrderSource;
use ulid::Ulid;

use super::amount::{Amount, TokenAmount};
use crate::primitives::{IntentId, MakerId, StrategyHash};
use crate::SolventError;

/// Server-assigned public trade id: a ULID, so it is opaque yet time-sortable — list pagination is
/// a plain `ORDER BY id`. Minted at the edge (a ULID needs the clock); core only carries the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TradeId(pub Ulid);

impl fmt::Display for TradeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl FromStr for TradeId {
    type Err = SolventError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s)
            .map(TradeId)
            .map_err(|e| SolventError::InvalidId {
                id_type: "TradeId",
                reason: e.to_string(),
            })
    }
}

/// Where a trade sits in its lifecycle. The happy path climbs `Created → … → Confirmed`; a trade
/// that never routes ends `Declined`, and one whose fill reverts ends `Failed`. Declaration order
/// is the monotonic rank the ledger's forward-only guard compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum TradeStatus {
    Created,
    Quoted,
    Reserved,
    Simulated,
    Submitted,
    Confirmed,
    Declined,
    Failed,
}

impl TradeStatus {
    /// The wire/DB spelling (lowercase). Paired with [`FromStr`]; the round-trip test guards them.
    pub fn as_str(self) -> &'static str {
        match self {
            TradeStatus::Created => "created",
            TradeStatus::Quoted => "quoted",
            TradeStatus::Reserved => "reserved",
            TradeStatus::Simulated => "simulated",
            TradeStatus::Submitted => "submitted",
            TradeStatus::Confirmed => "confirmed",
            TradeStatus::Declined => "declined",
            TradeStatus::Failed => "failed",
        }
    }

    /// Monotonic lifecycle rank (the declaration order): a persisted transition fires only when it
    /// moves forward, so a replayed or out-of-order update cannot walk a trade backwards.
    pub fn rank(self) -> u8 {
        self as u8
    }
}

impl fmt::Display for TradeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TradeStatus {
    type Err = SolventError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let status = match s {
            "created" => TradeStatus::Created,
            "quoted" => TradeStatus::Quoted,
            "reserved" => TradeStatus::Reserved,
            "simulated" => TradeStatus::Simulated,
            "submitted" => TradeStatus::Submitted,
            "confirmed" => TradeStatus::Confirmed,
            "declined" => TradeStatus::Declined,
            "failed" => TradeStatus::Failed,
            other => {
                return Err(SolventError::InvalidId {
                    id_type: "TradeStatus",
                    reason: format!("unknown status '{other}'"),
                })
            }
        };
        Ok(status)
    }
}

/// A submitted swap and its settlement so far. The signed UniswapX order is carried by the
/// `order_hash`/`signature`/`deadline_block` fields — a trade *contains* an order, it is not a
/// second name for one. The nullable fields fill in as the lifecycle advances.
#[derive(Debug, Clone, PartialEq)]
pub struct Trade {
    pub id: TradeId,
    /// The signed order hash — the idempotency anchor: one order maps to one trade.
    pub order_hash: IntentId,
    pub taker: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub min_amount_out: U256,
    /// The amount actually delivered, set once the fill confirms.
    pub amount_out: Option<U256>,
    pub status: TradeStatus,
    pub deadline_block: u64,
    /// The taker's signature over the order; absent when a trade is declined before signing.
    pub signature: Option<Bytes>,
    /// Price impact of the routed quote, in percent.
    pub price_impact_pct: Option<f64>,
    /// Expected resolver profit from exact-out routing, net of estimated gas, in `token_in` base units.
    pub surplus: Option<U256>,
    pub tx_hash: Option<B256>,
    pub block_number: Option<u64>,
    /// Unix seconds.
    pub created_at: u64,
    pub settled_at: Option<u64>,
    /// USD price of `token_in`/`token_out` captured when the trade was submitted, so fee and value
    /// figures stay fixed at trade-time economics rather than drifting with the current market.
    /// What sourcing the delivery would cost, gas included, in `token_in` — present even when the
    /// trade declined, since it is the only account a declined trade gives of itself. `None` when
    /// no candidate had capacity, so there was no cost to quote.
    pub indicative_amount_in: Option<U256>,
    /// Which venue the order arrived from.
    pub source: OrderSource,
    pub token_in_price_usd: Option<f64>,
    pub token_out_price_usd: Option<f64>,
}

/// One maker's slice of the routed split. The tokens and the settlement tx are the trade's — one
/// pair, one fill tx — so a leg holds only what is leg-specific: the maker, its strategy, and how
/// much of the split it carried. `amount_in`/`amount_out` come from routing; the display share is
/// derived at read time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeLeg {
    pub maker: MakerId,
    pub strategy_hash: StrategyHash,
    pub amount_in: U256,
    pub amount_out: U256,
}

/// One lifecycle stage a trade reached, and when. The detail timeline is these rows walked against
/// the fixed happy-path ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TradeAttempt {
    pub status: TradeStatus,
    /// Unix seconds.
    pub at: u64,
}

/// A trade with everything the detail view needs: its stage timeline and the makers it sourced.
#[derive(Debug, Clone, PartialEq)]
pub struct TradeInfo {
    pub trade: Trade,
    pub attempts: Vec<TradeAttempt>,
    pub legs: Vec<TradeLeg>,
}

/// One trade on the wire, header-only in a list; a detail also carries `lifecycle`, `legs`, and the
/// order's coordinates (omitted from JSON when absent). Assembled by the trade service.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TradeView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature_present: Option<bool>,
    pub id: String,
    pub status: String,
    #[schema(value_type = String)]
    pub taker: Address,
    /// The swapper's input token and the maximum it authorized.
    pub input: TokenAmount,
    /// The output token and the amount delivered (or the signed floor, until it settles).
    pub output: TokenAmount,
    /// Price impact of the routed quote, in percent; absent when no route was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_impact_pct: Option<f64>,
    /// Expected resolver profit from exact-out routing, net of estimated gas, in input-token units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surplus: Option<Amount>,
    /// Where the order came from: `uniswapx` (the public book) or `solvent` (our own endpoint).
    pub source: String,
    /// What sourcing the delivery would have cost, gas included, in input-token units — present on
    /// declined trades too, where it is the only account of why the resolver passed. Compare it to
    /// `input` to read the shortfall.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indicative_input: Option<Amount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_number: Option<u64>,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<u64>,
    /// The stage timeline — detail only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<Vec<TradeAction>>,
    /// The maker slices the trade sourced — detail only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legs: Option<Vec<MakerLeg>>,
    /// The signed order hash — detail only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_hash: Option<String>,
    /// Signed order expiry as Unix seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_block: Option<u64>,
}

/// One lifecycle stage a trade reached, and when (unix seconds).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TradeAction {
    pub status: String,
    pub at: u64,
}

/// One maker's slice of the routed split, on the wire.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MakerLeg {
    pub curve: Option<String>,
    pub maker: String,
    pub strategy_hash: String,
    pub amount_in: Amount,
    pub amount_out: Amount,
}

/// A settlement in a maker's feed: the trade header plus the maker's share and captured fee.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MakerTrade {
    #[serde(flatten)]
    pub trade: TradeView,
    /// The maker's slice of the trade (delivered ÷ total) — settled trades only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_pct: Option<f64>,
    /// The maker's captured fee, valued at the trade-time prices — settled trades only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_usd: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_str_round_trips_and_ranks_in_declaration_order() {
        let all = [
            TradeStatus::Created,
            TradeStatus::Quoted,
            TradeStatus::Reserved,
            TradeStatus::Simulated,
            TradeStatus::Submitted,
            TradeStatus::Confirmed,
            TradeStatus::Declined,
            TradeStatus::Failed,
        ];
        for (i, status) in all.iter().enumerate() {
            assert_eq!(TradeStatus::from_str(status.as_str()).unwrap(), *status);
            assert_eq!(usize::from(status.rank()), i);
        }
        // The happy path is strictly increasing, so the ledger's `status_rank < new` guard advances.
        assert!(TradeStatus::Created.rank() < TradeStatus::Reserved.rank());
        assert!(TradeStatus::Reserved.rank() < TradeStatus::Submitted.rank());
        assert!(TradeStatus::from_str("nope").is_err());
    }
}
