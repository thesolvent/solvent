//! Server configuration, loaded from a TOML file via the `config` crate, plus the startup error.

use std::net::SocketAddr;

use serde::Deserialize;
use solvent_adapters::http::state::{AppConfig, Features};
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
    #[serde(default = "default_fee_bps")]
    pub default_fee_bps: u32,
    #[serde(default = "default_explorer")]
    pub block_explorer_url: String,
    #[serde(default = "default_networks")]
    pub networks: Vec<String>,
    #[serde(default = "default_true")]
    pub faucet: bool,
}

impl Config {
    /// Load from `<path>.toml` (the `config` crate resolves the extension).
    pub fn load(path: &str) -> Result<Self, StartupError> {
        let loaded = config::Config::builder()
            .add_source(config::File::with_name(path))
            .build()?;
        Ok(loaded.try_deserialize()?)
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

/// A failure during boot; each aborts startup with a clear message.
#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("config: {0}")]
    Config(#[from] config::ConfigError),
    #[error("rpc url: {0}")]
    RpcUrl(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Solvent(#[from] SolventError),
}
