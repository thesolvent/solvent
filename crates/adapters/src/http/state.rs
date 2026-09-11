//! Handler context. `AppState` is the shared, cheaply-cloneable bundle every handler receives; the
//! app (composition root) builds it. `AppConfig` doubles as the `/config` response payload.

use std::sync::Arc;

use super::depth::DepthReader;
use alloy::primitives::Address;
use serde::Serialize;
use solvent_core::asset::{AssetManager, PairHistoryService};
use solvent_core::balances::BalancesService;
use solvent_core::deps::quote_log::QuoteLog;
use solvent_core::deps::registry::EventStore;
use solvent_core::maker::MakerService;
use solvent_core::pool::PoolService;
use solvent_core::quote::QuoteService;
use solvent_core::rebate::RebateService;
use solvent_core::registry::SharedSnapshot;
use solvent_core::swap::SwapService;
use solvent_core::trade::TradeService;
use solvent_core::valuation::Valuation;

use crate::chain::ChainHead;
use crate::ingest::uniswapx::{FeedHealth, ServerCosigner, UniswapXV2Normalizer};
use crate::metrics::SqliteOrderLog;

/// Feature flags the FE reads at bootstrap. `earn` / `send_buy` are always off in the MVP.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Features {
    pub faucet: bool,
    pub earn: bool,
    pub send_buy: bool,
}

/// Runtime config the FE reads instead of hardcoding — also the `/config` response body.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AppConfig {
    pub chain_id: u64,
    pub features: Features,
    pub default_fee_bps: u32,
    pub networks: Vec<String>,
    pub block_explorer_url: String,
    /// The Aqua deployment holding maker virtual balances.
    #[schema(value_type = String)]
    pub aqua: Address,
    /// The application whose strategies Aqua scopes independently.
    #[schema(value_type = String)]
    pub app: Address,
    /// The UniswapX reactor that settles taker orders.
    #[schema(value_type = String)]
    pub reactor: Address,
    /// The Permit2 contract verifying the taker witness.
    #[schema(value_type = String)]
    pub permit2: Address,
    /// The public executor target for encoded rebate transactions.
    #[schema(value_type = String)]
    pub filler: Address,
    /// The immutable token whose balance gates every strategy to the filler contract.
    #[schema(value_type = String)]
    pub taker_credential: Address,
    /// The resolver authorized to cosign taker orders.
    #[schema(value_type = String)]
    pub cosigner: Address,
}

/// Shared handler context: the config payload, a chain provider (block height), and the registry
/// snapshot handle (read lock-free via [`SharedSnapshot::load`]).
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub head: ChainHead,
    pub assets: Arc<AssetManager>,
    pub pair_history: Arc<PairHistoryService>,
    pub pools: Arc<PoolService>,
    pub depth: DepthReader,
    pub balances: Arc<BalancesService>,
    /// The maker read-surface: positions, the positions list, and the roster.
    pub makers: Arc<MakerService>,
    pub quote: Arc<QuoteService>,
    pub swap: Arc<SwapService>,
    /// Durable, signed rebate work exposed through the public read-only queue.
    pub rebates: Arc<RebateService>,
    /// Cosigns taker-signed orders on the swap path (holds only the resolver's cosigner key).
    pub cosigner: Arc<ServerCosigner>,
    /// Decodes and validates the orders this server itself cosigns, so the self-venue path applies
    /// the same admission rules as the feed. Its cosigner allow-list is our own key, not Uniswap's.
    pub normalizer: Arc<UniswapXV2Normalizer>,
    /// The trade read-surface, backing the `/trades` and maker-settlements endpoints.
    pub trades: Arc<TradeService>,
    /// The live registry snapshot — the stat tiles read active-maker counts lock-free.
    pub registry: Arc<SharedSnapshot>,
    /// The durable Aqua event log, read by the `/activity` feed.
    pub registry_store: Arc<dyn EventStore>,
    /// USD valuation + market data (price, 24h change) — the read DTOs are valued through this.
    pub valuation: Arc<Valuation>,
    /// Records each served quote, for maker uptime / latency / fill-share analytics.
    pub quote_log: Arc<dyn QuoteLog>,
    /// Liveness of the order feed, when one is configured. `None` means the resolver takes orders
    /// only from its own submit path, so there is no feed to be stale.
    pub feed_health: Option<Arc<FeedHealth>>,
    /// Every order the feed showed us, for the explorer's order list. `None` leaves it empty.
    pub order_log: Option<Arc<SqliteOrderLog>>,
}
