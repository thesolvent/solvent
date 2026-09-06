//! Server configuration, loaded from a TOML file via the `config` crate, plus the startup error.

use std::collections::HashMap;
use std::net::SocketAddr;

use alloy::primitives::{address, Address};
use serde::Deserialize;
use solvent_adapters::http::state::{AppConfig, Features};
use solvent_core::asset::TokenList;
use solvent_core::SolventError;

/// Everything the server needs to boot, deserialized from the config file. `database_url` is
/// optional: absent starts with an empty registry snapshot (the M2 worker hydrates it), present
/// hydrates from that store at boot.
#[derive(Debug, Deserialize)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub rpc_url: String,
    #[serde(default)]
    pub database_url: Option<String>,
    pub chain_id: u64,
    /// The Aqua contract (allowance spender) and the SwapVM router (`app`, which keys strategies in
    /// the registry). The depth reader needs both to read each maker's pullable wallet on chain.
    pub aqua_address: Address,
    pub app_address: Address,
    #[serde(default = "default_fee_bps")]
    pub default_fee_bps: u32,
    #[serde(default = "default_explorer")]
    pub block_explorer_url: String,
    #[serde(default = "default_networks")]
    pub networks: Vec<String>,
    #[serde(default = "default_true")]
    pub faucet: bool,
    #[serde(default = "default_token_list")]
    pub token_list: String,
    /// The native gas token (WETH), whose USD price values the per-leg gas cost. Unset (zero) routes
    /// gas-free until a price is configured.
    #[serde(default)]
    pub native_token: Address,
    /// Binance combined-stream WS base for the token-price feed.
    #[serde(default = "default_binance_ws")]
    pub binance_ws_url: String,
    /// Gas a single fill leg costs on this chain, in gas units — sizes the sparsity threshold.
    #[serde(default = "default_gas_units")]
    pub gas_units_per_leg: u64,
    /// Binance symbol → the tokens it prices (e.g. `ETHUSDT` → `[WETH]`). Drives the price feed.
    #[serde(default)]
    pub price_symbols: Vec<PriceSymbol>,
    /// The resolver's Aqua filler contract the swap path fills through.
    #[serde(default)]
    pub filler: Address,
    /// The canonical Permit2 (same on every chain); overridable for a bespoke devnet deploy.
    #[serde(default = "default_permit2")]
    pub permit2: Address,
    /// Confirmations a fill waits for before it settles.
    #[serde(default = "default_confirmations")]
    pub confirmations: u64,
    /// How long a reservation holds before the TTL sweep may release it.
    #[serde(default = "default_ttl_secs")]
    pub reservation_ttl_secs: u64,
    /// The cosigner's decay window applied to each order.
    #[serde(default = "default_decay_secs")]
    pub decay_window_secs: u64,
    /// Path to the tx engine's durable state (redb), so in-flight fills survive a restart.
    #[serde(default = "default_wallet_state_db")]
    pub wallet_state_db: String,
}

/// One Binance price symbol and the tokens whose USD price it feeds.
#[derive(Debug, Deserialize)]
pub struct PriceSymbol {
    pub symbol: String,
    pub tokens: Vec<Address>,
}

impl Config {
    /// Load from `<path>.toml` (the `config` crate resolves the extension).
    pub fn load(path: &str) -> Result<Self, StartupError> {
        let loaded = config::Config::builder()
            .add_source(config::File::with_name(path))
            .build()?;
        Ok(loaded.try_deserialize()?)
    }

    /// The price feed's `symbol → tokens` map, as the `BinanceFeed` consumes it.
    pub fn price_feed_symbols(&self) -> HashMap<String, Vec<Address>> {
        self.price_symbols
            .iter()
            .map(|entry| (entry.symbol.clone(), entry.tokens.clone()))
            .collect()
    }

    /// The subset the FE reads at bootstrap (the `/config` payload). `earn`/`send_buy` are MVP-off.
    pub fn app_config(&self) -> AppConfig {
        AppConfig {
            chain_id: self.chain_id,
            features: Features {
                faucet: self.faucet,
                earn: false,
                send_buy: false,
            },
            default_fee_bps: self.default_fee_bps,
            networks: self.networks.clone(),
            block_explorer_url: self.block_explorer_url.clone(),
        }
    }
}

fn default_fee_bps() -> u32 {
    5
}
fn default_explorer() -> String {
    "http://localhost:5100".to_string()
}
fn default_networks() -> Vec<String> {
    vec!["Ethereum".to_string()]
}
fn default_true() -> bool {
    true
}
fn default_token_list() -> String {
    "tokens.devnet.json".to_string()
}
fn default_binance_ws() -> String {
    "wss://stream.binance.com:9443".to_string()
}
fn default_gas_units() -> u64 {
    150_000
}
fn default_permit2() -> Address {
    address!("000000000022D473030F116dDEE9F6B43aC78BA3")
}
fn default_confirmations() -> u64 {
    1
}
fn default_ttl_secs() -> u64 {
    60
}
fn default_decay_secs() -> u64 {
    60
}
fn default_wallet_state_db() -> String {
    "walletkit.redb".to_string()
}

/// Read and parse the token list JSON at `path`.
pub fn load_token_list(path: &str) -> Result<TokenList, StartupError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| StartupError::TokenList(format!("{path}: {e}")))?;
    serde_json::from_str(&text).map_err(|e| StartupError::TokenList(format!("{path}: {e}")))
}

/// A failure during boot; each aborts startup with a clear message.
#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("config: {0}")]
    Config(#[from] config::ConfigError),
    #[error("rpc url: {0}")]
    RpcUrl(String),
    #[error("database_url is required to run the live server")]
    MissingDatabase,
    #[error("{0} must be set (the resolver's signing keys are read from the environment)")]
    MissingSecret(&'static str),
    #[error("bad signing key: {0}")]
    Key(String),
    #[error("wallet state store: {0}")]
    WalletStore(String),
    #[error("token list: {0}")]
    TokenList(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Solvent(#[from] SolventError),
}
