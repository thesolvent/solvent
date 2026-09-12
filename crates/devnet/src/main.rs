//! Devnet faucet: drips the seeded test tokens (and tops up gas) to any address, rate-limited per
//! address. Reads the deploy manifest for token addresses; RPC + signer come from the environment.
//! Devnet only — the minter key is a well-known throwaway and `mint` is unrestricted.

mod cooldown;
mod faucet;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::U256;
use alloy::providers::{Provider, ProviderBuilder};
use walletkit::adapters::policy::{AllowAll, DefaultPolicyEngine};
use walletkit::adapters::{LocalSigner, RedbStateStore, Transport};
use walletkit::Wallet;

use crate::cooldown::Cooldown;
use crate::faucet::{router, AppState, Manifest, StartupError};

const RECEIPT_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[tokio::main]
async fn main() -> Result<(), StartupError> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let cfg = Config::from_env()?;
    let manifest = Manifest::load(&cfg.manifest_path)?;

    let read_rpc = cfg
        .rpc_url
        .parse()
        .map_err(|e| StartupError::Config(format!("RPC_URL: {e}")))?;
    let provider = ProviderBuilder::new().connect_http(read_rpc);
    provider.client().set_poll_interval(RECEIPT_POLL_INTERVAL);
    let transport = Transport::url(
        cfg.rpc_url
            .parse()
            .map_err(|e| StartupError::Config(format!("RPC_URL: {e}")))?,
    )
    .map_err(|e| StartupError::Config(format!("RPC_URL: {e}")))?;
    let signer = LocalSigner::from_private_key(&cfg.private_key)
        .map_err(|e| StartupError::Config(format!("FAUCET_PRIVATE_KEY: {e}")))?;
    let policy = DefaultPolicyEngine::new(
        vec![Box::new(AllowAll)],
        Arc::new(walletkit::adapters::SystemClock),
    );
    let state_store = RedbStateStore::open(&cfg.wallet_state_db)
        .map_err(|e| StartupError::WalletStore(e.to_string()))?;
    let wallet = Arc::new(
        Wallet::builder(Arc::new(transport), Arc::new(signer), Arc::new(policy))
            .store(Arc::new(state_store))
            .confirmations(1)
            .bump_timeout(30)
            .build(),
    );
    let _wallet_runner = Arc::clone(&wallet).run(RECEIPT_POLL_INTERVAL);

    let state = AppState::new(
        provider.erased(),
        wallet,
        cfg.chain_id,
        manifest,
        Cooldown::new(cfg.cooldown),
        cfg.drip_units,
        cfg.gas_target_wei,
    );

    let listener = tokio::net::TcpListener::bind(cfg.bind_addr).await?;
    tracing::info!(addr = %cfg.bind_addr, tokens = state.token_count(), "faucet listening");
    axum::serve(listener, router(Arc::new(state))).await?;
    Ok(())
}

/// Faucet configuration, all from the environment. `private_key` is a secret — it is never logged
/// and lives only long enough to build the signer.
struct Config {
    rpc_url: String,
    private_key: String,
    manifest_path: String,
    wallet_state_db: String,
    chain_id: u64,
    bind_addr: SocketAddr,
    cooldown: Duration,
    drip_units: u64,
    gas_target_wei: U256,
}

impl Config {
    fn from_env() -> Result<Self, StartupError> {
        let required = |k: &str| {
            std::env::var(k).map_err(|_| StartupError::Config(format!("{k} is required")))
        };
        let optional =
            |k: &str, default: &str| std::env::var(k).unwrap_or_else(|_| default.to_string());

        let bind_addr: SocketAddr = optional("BIND_ADDR", "0.0.0.0:8080")
            .parse()
            .map_err(|e| StartupError::Config(format!("BIND_ADDR: {e}")))?;
        let cooldown_secs: u64 = optional("COOLDOWN_SECS", "1")
            .parse()
            .map_err(|e| StartupError::Config(format!("COOLDOWN_SECS: {e}")))?;
        let drip_units: u64 = optional("DRIP_UNITS", "1000")
            .parse()
            .map_err(|e| StartupError::Config(format!("DRIP_UNITS: {e}")))?;
        let gas_target_eth: u64 = optional("GAS_TARGET_ETH", "1")
            .parse()
            .map_err(|e| StartupError::Config(format!("GAS_TARGET_ETH: {e}")))?;
        let chain_id: u64 = optional("CHAIN_ID", "31337")
            .parse()
            .map_err(|e| StartupError::Config(format!("CHAIN_ID: {e}")))?;

        Ok(Config {
            rpc_url: required("RPC_URL")?,
            private_key: required("FAUCET_PRIVATE_KEY")?,
            manifest_path: optional("MANIFEST_PATH", "deployments/solvent-devnet.json"),
            wallet_state_db: optional("FAUCET_WALLET_STATE_DB", "faucet-walletkit.redb"),
            chain_id,
            bind_addr,
            cooldown: Duration::from_secs(cooldown_secs),
            drip_units,
            gas_target_wei: U256::from(gas_target_eth) * U256::from(10u64).pow(U256::from(18u64)),
        })
    }
}
