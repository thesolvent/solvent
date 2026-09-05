//! Handler context. `AppState` is the shared, cheaply-cloneable bundle every handler receives; the
//! app (composition root) builds it. `AppConfig` doubles as the `/config` response payload.

use std::sync::Arc;

use serde::Serialize;
use solvent_core::asset::AssetManager;

use crate::chain::ChainHead;

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
}
