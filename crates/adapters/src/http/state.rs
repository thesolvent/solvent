//! Handler context. `AppState` is the shared, cheaply-cloneable bundle every handler receives; the
//! app (composition root) builds it. `AppConfig` doubles as the `/config` response payload.

use std::sync::Arc;

use serde::Serialize;
use solvent_core::asset::AssetManager;
use solvent_core::balances::BalancesService;
use solvent_core::deps::quote_log::QuoteLog;
use solvent_core::deps::registry::EventStore;
use solvent_core::maker::MakerService;
use solvent_core::pool::{DepthService, PoolService};
use solvent_core::quote::QuoteService;
use solvent_core::registry::SharedSnapshot;
use solvent_core::swap::SwapService;
use solvent_core::trade::TradeService;
use solvent_core::valuation::Valuation;

use crate::chain::ChainHead;
use crate::ingest::uniswapx::ServerCosigner;

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
}

/// Shared handler context: the config payload, a chain provider (block height), and the registry
/// snapshot handle (read lock-free via [`SharedSnapshot::load`]).
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub head: ChainHead,
    pub assets: Arc<AssetManager>,
    pub pools: Arc<PoolService>,
    pub depth: Arc<DepthService>,
    pub balances: Arc<BalancesService>,
    /// The maker read-surface: positions, the positions list, and the roster.
    pub makers: Arc<MakerService>,
    pub quote: Arc<QuoteService>,
    pub swap: Arc<SwapService>,
    /// Cosigns taker-signed orders on the swap path (holds only the resolver's cosigner key).
    pub cosigner: Arc<ServerCosigner>,
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
}
