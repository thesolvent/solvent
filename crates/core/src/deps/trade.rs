//! The trade store port: durable, idempotent persistence of the trade lifecycle. `create` dedups on
//! the signed order hash; `advance`/`settle` move the lifecycle forward, guarded so a replay is a
//! no-op.

use alloy_primitives::{Address, B256, U256};
use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::trade::{Trade, TradeAttempt, TradeId, TradeInfo, TradeLeg, TradeStatus};
use crate::primitives::{IntentId, StrategyHash};

/// The outcome of a [`create`](TradeStore::create): the trade id now representing this order, and
/// whether it was newly inserted (vs. an existing trade for the same order hash).
pub struct CreateResult {
    pub id: TradeId,
    pub created: bool,
}

/// A trade's terminal settlement — the filled amount and on-chain coordinates, applied once.
pub struct Settlement {
    pub status: TradeStatus,
    /// Why a terminal trade never filled — a rendered `DeclineReason`; `None` on the happy path.
    pub reason: Option<String>,
    pub amount_out: Option<U256>,
    pub tx_hash: Option<B256>,
    pub block_number: Option<u64>,
    /// Unix seconds.
    pub at: u64,
}

/// Server-side list filters. `pair` matches either token orientation.
#[derive(Default)]
pub struct TradeFilter {
    pub status: Option<TradeStatus>,
    pub taker: Option<Address>,
    pub pair: Option<(Address, Address)>,
    pub strategy_hash: Option<StrategyHash>,
}

/// One page of a trade listing: newest first, `cursor` the last id of the previous page.
pub struct Page {
    pub limit: u32,
    pub cursor: Option<TradeId>,
}

/// A trade paired with the amounts supplied by all of one maker's strategies.
#[non_exhaustive]
pub struct MakerFill {
    pub trade: Trade,
    pub amount_in: U256,
    pub amount_out: U256,
}

impl MakerFill {
    pub fn new(trade: Trade, amount_in: U256, amount_out: U256) -> Self {
        Self {
            trade,
            amount_in,
            amount_out,
        }
    }
}

/// Aggregate counts over the trade table, for the stat tiles. `median_impact_pct` is `None` until
/// trades carry a price impact.
pub struct TradeStats {
    /// Trades that reached a terminal (settled) state.
    pub settled: u64,
    /// Of those, how many confirmed on-chain.
    pub confirmed: u64,
    /// Of those, how many failed on-chain — the confirmation-rate denominator with `confirmed`
    /// (declines are settled but excluded).
    pub failed: u64,
    pub median_impact_pct: Option<f64>,
}

#[async_trait]
pub trait TradeStore: Send + Sync {
    /// Persist a new trade with its legs and initial timeline, atomically. Idempotent on the order
    /// hash: a resubmit returns the existing id with `created = false` and writes nothing new.
    async fn create(
        &self,
        trade: &Trade,
        legs: &[TradeLeg],
        attempts: &[TradeAttempt],
    ) -> Result<CreateResult, TradeStoreError>;

    /// Advance a trade to an intermediate stage: append the attempt and move the status forward. A
    /// backward or duplicate transition is a no-op (the rank guard + the attempt key).
    async fn advance(
        &self,
        id: &TradeId,
        status: TradeStatus,
        at: u64,
    ) -> Result<(), TradeStoreError>;

    /// Apply a trade's terminal settlement. A no-op once already settled, so a replay is safe.
    async fn settle(&self, id: &TradeId, outcome: &Settlement) -> Result<(), TradeStoreError>;

    /// Full detail for one trade — header, stage timeline, and legs — or `None` if unknown.
    async fn info(&self, id: &TradeId) -> Result<Option<TradeInfo>, TradeStoreError>;

    /// The trade recorded for a signed order hash, or `None` — the reconcile path's bridge from a
    /// settled fill (keyed by the order hash) back to its trade.
    async fn find_by_order(&self, order_hash: &IntentId) -> Result<Option<Trade>, TradeStoreError>;

    /// A page of trade headers (newest first) matching `filter`.
    async fn list(&self, filter: &TradeFilter, page: &Page) -> Result<Vec<Trade>, TradeStoreError>;

    /// A page of a maker's trades (newest first), with its combined amounts across strategies.
    async fn list_for_maker(
        &self,
        maker: Address,
        page: &Page,
        window: Option<std::ops::Range<u64>>,
    ) -> Result<Vec<MakerFill>, TradeStoreError>;

    /// Aggregate lifecycle counts for the stat tiles.
    async fn stats(&self) -> Result<TradeStats, TradeStoreError>;
}

/// A trade store failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TradeStoreError {
    /// The database call or a payload conversion failed.
    #[error("db: {0}")]
    Db(String),
}
