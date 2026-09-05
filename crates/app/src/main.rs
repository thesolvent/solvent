//! Solvent server — the composition root. Wires the config, the registry snapshot (hydrated from the
//! durable event log), a chain provider, and the HTTP adapter into a running axum server.

mod config;

use std::sync::Arc;
use std::time::Duration;

use alloy::providers::{Provider, ProviderBuilder};
use solvent_adapters::chain::ChainHead;
use solvent_adapters::http::state::AppState;
use solvent_adapters::http::{self};
use solvent_adapters::registry::SqliteStore;
use solvent_core::asset::AssetManager;
use solvent_core::deps::registry::Store;
use solvent_core::primitives::registry::Snapshot;
use solvent_core::primitives::ChainId;
use solvent_core::registry::SharedSnapshot;
use solvent_core::SolventError;
use sqlx::SqlitePool;

use crate::config::{load_token_list, Config, StartupError};

/// How often the background poller refreshes the cached chain head.
const BLOCK_POLL_INTERVAL: Duration = Duration::from_secs(2);

#[tokio::main]
async fn main() -> Result<(), StartupError> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config_path = std::env::var("SOLVENT_CONFIG").unwrap_or_else(|_| "solvent".to_string());
    let config = Config::load(&config_path)?;

    let rpc = config
        .rpc_url
        .parse()
        .map_err(|e| StartupError::RpcUrl(format!("{e}")))?;
    let provider = ProviderBuilder::new().connect_http(rpc).erased();
    let (head, poller) = ChainHead::new(provider, BLOCK_POLL_INTERVAL);
    tokio::spawn(poller);

    let registry =
        Arc::new(hydrate(config.database_url.as_deref(), ChainId(config.chain_id)).await?);
    let assets = Arc::new(AssetManager::new(
        load_token_list(&config.token_list)?,
        registry,
    ));

    let state = AppState {
        config: Arc::new(config.app_config()),
        head,
        assets,
    };

    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(addr = %config.bind_addr, "solvent listening");
    axum::serve(listener, http::router(state))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

/// Rebuild the registry snapshot from the durable event log, or start empty when no store is set.
async fn hydrate(db_url: Option<&str>, chain: ChainId) -> Result<SharedSnapshot, StartupError> {
    let Some(url) = db_url else {
        return Ok(SharedSnapshot::default());
    };
    let store = SqliteStore::new(SqlitePool::connect(url).await?);
    store.migrate().await.map_err(SolventError::from)?;
    let mut snapshot = Snapshot::default();
    for ext in store.events(chain).await.map_err(SolventError::from)? {
        snapshot.apply(ext.event);
    }
    Ok(SharedSnapshot::new(snapshot))
}

/// Resolve when the process is asked to stop (Ctrl-C), so in-flight requests can drain.
async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
