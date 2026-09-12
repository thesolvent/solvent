//! Server configuration, loaded from a TOML file via the `config` crate, plus the startup error.

use std::collections::HashMap;
use std::net::SocketAddr;

use alloy::primitives::{address, Address, B256};
use serde::Deserialize;
use solvent_adapters::http::state::{AppConfig, Features};
use solvent_core::asset::TokenList;
use solvent_core::SolventError;

/// Everything the server needs to boot, deserialized from the config file. `database_url` is
/// optional: absent starts with an empty registry snapshot (a live watcher hydrates it), present
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
    /// Binance Spot REST base for historical klines used only by charts.
    #[serde(default = "default_binance_rest")]
    pub binance_rest_url: String,
    /// Gas a single fill leg costs on this chain, in gas units — sizes the sparsity threshold.
    #[serde(default = "default_gas_units")]
    pub gas_units_per_leg: u64,
    /// Binance symbol → the tokens it prices (e.g. `ETHUSDT` → `[WETH]`). Drives the price feed.
    #[serde(default)]
    pub price_symbols: Vec<PriceSymbol>,
    /// USD stablecoins pegged to $1 with no Binance pair (USDT — the quote unit). Seeded into the
    /// price cache at boot so every supported asset values.
    #[serde(default)]
    pub usd_stable_pegs: Vec<Address>,
    /// The resolver's Aqua filler contract the swap path fills through.
    #[serde(default)]
    pub filler: Address,
    /// Same-chain ERC-7683 contracts deployed with the filler stack.
    #[serde(default)]
    pub erc7683_settler: Address,
    #[serde(default)]
    pub erc7683_filler: Address,
    #[serde(default)]
    pub erc7683_resolver: Address,
    #[serde(default = "default_erc7683_executor_fee_bps")]
    pub erc7683_executor_fee_bps: u32,
    /// The UniswapX reactor a taker's order settles through; published so a client can name it.
    pub reactor: Address,
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
    /// Optional private listener and allow-listed contracts for cross-chain coordination.
    #[serde(default)]
    pub crosschain: Option<CrossChainConfig>,
    #[serde(default)]
    pub rebate: RebateConfig,
    /// Block the registry watcher starts scanning from — the Aqua deployment, since no strategy can
    /// exist before it. Left at zero it re-scans the whole chain on a cold store, which on mainnet
    /// is thousands of `getLogs` calls over blocks that cannot contain an event.
    #[serde(default)]
    pub registry_start_block: u64,
    /// Blocks per `getLogs` request when the watcher scans. Nodes cap a response at a fixed number
    /// of logs, and Aqua is dense enough on mainnet that the indexer's default span exceeds it.
    /// Unset leaves the indexer's own default.
    #[serde(default)]
    pub registry_scan_span: Option<u64>,

    /// Root of the Orders API the live feed polls, or a local mirror serving the same shape. Unset
    /// leaves the feed off and the resolver takes orders only from its own submit endpoint.
    #[serde(default)]
    pub orders_api_url: Option<String>,
    /// The order type to ask for. UniswapX runs a different reactor and decay model per chain, and
    /// this resolver decodes V2 Dutch orders, which mainnet serves.
    #[serde(default = "default_order_type")]
    pub order_type: String,
    /// Gap between individual order-feed requests. The endpoint allows four per second; one request
    /// leaves per interval however many scopes are polled.
    #[serde(default = "default_order_poll_ms")]
    pub order_poll_ms: u64,
    /// How long the feed may fail to reach the endpoint before readiness turns false.
    #[serde(default = "default_feed_silence_secs")]
    pub feed_silence_secs: u64,
    /// Cosigner keys whose signature the feed's orders must carry. The reactor only checks that a
    /// cosignature matches the order's own `cosigner` field, so pinning the identity is ours to do;
    /// on mainnet this is Uniswap's operational key.
    #[serde(default)]
    pub expected_cosigners: Vec<Address>,
    /// Tokens the resolver will touch on either side of an order. Empty falls back to the token
    /// list, which is the set the registry can price anyway.
    #[serde(default)]
    pub admitted_tokens: Vec<Address>,
    /// Ceiling on an order's output legs.
    #[serde(default = "default_max_outputs")]
    pub max_outputs: usize,
    /// Order hashes held against re-delivery. Each poll returns the same newest page, so this needs
    /// to outrun the page size by a wide margin, not the order rate.
    #[serde(default = "default_dedup_capacity")]
    pub dedup_capacity: u64,
    /// Ceiling on orders held for re-pricing at once.
    #[serde(default = "default_max_tracked")]
    pub max_tracked_intents: usize,
    /// How long a hash stays deduped. Longer than any order lives, so a single order is admitted
    /// once however many polls return it.
    #[serde(default = "default_dedup_ttl_secs")]
    pub dedup_ttl_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct CrossChainConfig {
    pub bind_addr: SocketAddr,
    #[serde(default)]
    pub destination_app: Address,
    #[serde(default)]
    pub origin_settler: Address,
    #[serde(default)]
    pub proof_outbox: Address,
    #[serde(default = "default_crosschain_quote_ttl_secs")]
    pub quote_ttl_secs: u64,
    #[serde(default)]
    pub direct_author: Option<DirectAuthorConfig>,
}

#[derive(Debug, Deserialize)]
pub struct DirectAuthorConfig {
    pub origin_chain_id: u64,
    pub origin_proof_outbox: Address,
    pub destination_proof_outbox: Address,
    pub origin_strategy_hash: B256,
}

/// One Binance price symbol and the tokens whose USD price it feeds.
///
/// Unknown fields are refused because TOML scopes bare keys to the table header above them: a
/// top-level key written below `[[price_symbols]]` becomes a field of that entry, and without this
/// it would be accepted and discarded, disabling whatever it configured with nothing logged.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceSymbol {
    pub symbol: String,
    pub tokens: Vec<Address>,
}

#[derive(Debug, Deserialize)]
pub struct RebateConfig {
    #[serde(default)]
    pub start_block: u64,
    #[serde(default = "default_rebate_deviation_bps")]
    pub deviation_threshold_bps: u64,
    #[serde(default = "default_rebate_gas_safety_bps")]
    pub gas_safety_bps: u64,
    #[serde(default = "default_rebate_gas_units")]
    pub gas_units: u64,
    #[serde(default = "default_rebate_market_max_age_secs")]
    pub market_max_age_secs: u64,
    #[serde(default = "default_rebate_authorization_ttl_blocks")]
    pub authorization_ttl_blocks: u64,
}

#[derive(Clone, Copy)]
pub struct Erc7683Contracts {
    pub settler: Address,
    pub filler: Address,
    pub resolver: Address,
}

impl Default for RebateConfig {
    fn default() -> Self {
        Self {
            start_block: 0,
            deviation_threshold_bps: default_rebate_deviation_bps(),
            gas_safety_bps: default_rebate_gas_safety_bps(),
            gas_units: default_rebate_gas_units(),
            market_max_age_secs: default_rebate_market_max_age_secs(),
            authorization_ttl_blocks: default_rebate_authorization_ttl_blocks(),
        }
    }
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
    pub fn app_config(
        &self,
        cosigner: Address,
        taker_credential: Address,
        erc7683: Option<Erc7683Contracts>,
    ) -> AppConfig {
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
            aqua: self.aqua_address,
            app: self.app_address,
            reactor: self.reactor,
            permit2: self.permit2,
            filler: self.filler,
            erc7683_settler: erc7683.map(|contracts| contracts.settler),
            erc7683_filler: erc7683.map(|contracts| contracts.filler),
            erc7683_resolver: erc7683.map(|contracts| contracts.resolver),
            taker_credential,
            cosigner,
        }
    }

    pub fn erc7683_contracts(&self) -> Result<Option<Erc7683Contracts>, StartupError> {
        let configured = [
            self.erc7683_settler,
            self.erc7683_filler,
            self.erc7683_resolver,
        ];
        if configured.iter().all(|address| address.is_zero()) {
            return Ok(None);
        }
        if configured.iter().any(|address| address.is_zero()) {
            return Err(StartupError::FillerConfiguration(
                "ERC-7683 settler, filler, and resolver must be configured together".to_string(),
            ));
        }
        Ok(Some(Erc7683Contracts {
            settler: self.erc7683_settler,
            filler: self.erc7683_filler,
            resolver: self.erc7683_resolver,
        }))
    }
}

fn default_fee_bps() -> u32 {
    5
}
fn default_erc7683_executor_fee_bps() -> u32 {
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
fn default_binance_rest() -> String {
    "https://api.binance.com".to_string()
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
fn default_order_type() -> String {
    "Dutch_V2".to_string()
}

/// Four requests per second is the endpoint's published ceiling.
fn default_order_poll_ms() -> u64 {
    250
}

fn default_feed_silence_secs() -> u64 {
    300
}

fn default_max_outputs() -> usize {
    4
}

fn default_max_tracked() -> usize {
    64
}

fn default_dedup_capacity() -> u64 {
    16_384
}

fn default_dedup_ttl_secs() -> u64 {
    900
}

fn default_wallet_state_db() -> String {
    "walletkit.redb".to_string()
}
fn default_crosschain_quote_ttl_secs() -> u64 {
    300
}
fn default_rebate_deviation_bps() -> u64 {
    50
}
fn default_rebate_gas_safety_bps() -> u64 {
    12_000
}
fn default_rebate_gas_units() -> u64 {
    350_000
}
fn default_rebate_market_max_age_secs() -> u64 {
    10
}
fn default_rebate_authorization_ttl_blocks() -> u64 {
    30
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
    #[error("filler configuration: {0}")]
    FillerConfiguration(String),
    #[error("orders feed: {0}")]
    OrdersFeed(String),
    #[error("token list: {0}")]
    TokenList(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Solvent(#[from] SolventError),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped example is the template every deployment copies, so a key in the wrong TOML
    /// scope ships the feature off everywhere. Every value here also equals its `serde` default,
    /// which is why this asserts placement in the parsed tree rather than the loaded values.
    #[test]
    fn the_example_config_puts_every_feed_key_at_the_root() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../solvent.example.toml");
        let raw = config::Config::builder()
            .add_source(config::File::with_name(path))
            .build()
            .expect("the shipped example is valid TOML");
        for key in [
            "order_type",
            "order_poll_ms",
            "feed_silence_secs",
            "expected_cosigners",
            "admitted_tokens",
            "max_outputs",
            "dedup_ttl_secs",
            "dedup_capacity",
            "max_tracked_intents",
        ] {
            assert!(
                raw.get::<config::Value>(key).is_ok(),
                "{key} is not at the root of the example config"
            );
        }
        Config::load(path).expect("the shipped example deserializes");
    }
}
