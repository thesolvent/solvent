//! Cross-chain composition root. It owns only HTTP clients and saga persistence, never an RPC or key.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::Address;
use serde::Deserialize;
use solvent_adapters::crosschain::{
    CircleCctpCompletion, CircleIrisClient, SolventClient, SqliteSagaStore,
};
use solvent_adapters::http::crosschain_proxy_router;
use solvent_adapters::ledger::SystemClock;
use solvent_core::crosschain::CrossChainProxy;
use solvent_core::deps::crosschain::{CctpAttestation, CctpCompletion, RemoteSolvent, SagaStore};
use solvent_core::obs::{info, warn};
use sqlx::SqlitePool;

#[derive(Debug, Deserialize)]
struct ProxyConfig {
    bind_addr: SocketAddr,
    database_url: String,
    origin_service_url: String,
    destination_service_url: String,
    #[serde(default = "default_poll_interval_ms")]
    poll_interval_ms: u64,
    #[serde(default)]
    cctp: Option<CctpProxyConfig>,
}

#[derive(Debug, Deserialize)]
struct CctpProxyConfig {
    iris_url: String,
    source_domain: u32,
    destination_domain: u32,
    finality_threshold: u32,
    destination_app: Address,
}

impl ProxyConfig {
    fn load(path: &str) -> Result<Self, ProxyStartupError> {
        Ok(config::Config::builder()
            .add_source(config::File::with_name(path))
            .build()?
            .try_deserialize()?)
    }
}

#[derive(Debug, thiserror::Error)]
enum ProxyStartupError {
    #[error("config: {0}")]
    Config(#[from] config::ConfigError),
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
    #[error("cross-chain service: {0}")]
    Service(#[from] solvent_core::SolventError),
    #[error("remote service configuration: {0}")]
    Remote(#[from] solvent_core::deps::crosschain::RemoteSolventError),
    #[error("Circle configuration: {0}")]
    Cctp(#[from] solvent_core::deps::crosschain::CctpAttestationError),
    #[error("server: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0} must be set for private service authentication")]
    MissingSecret(&'static str),
}

#[tokio::main]
async fn main() -> Result<(), ProxyStartupError> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let path =
        std::env::var("SOLVENT_PROXY_CONFIG").unwrap_or_else(|_| "solvent-proxy".to_string());
    let config = ProxyConfig::load(&path)?;
    let pool = SqlitePool::connect(&config.database_url).await?;
    let saga_store = Arc::new(SqliteSagaStore::new(pool));
    saga_store
        .migrate()
        .await
        .map_err(solvent_core::SolventError::from)?;
    let sagas: Arc<dyn SagaStore> = saga_store;
    let internal_token = std::env::var("SOLVENT_INTERNAL_TOKEN")
        .map_err(|_| ProxyStartupError::MissingSecret("SOLVENT_INTERNAL_TOKEN"))?;
    let origin: Arc<dyn RemoteSolvent> = Arc::new(SolventClient::from_base_url(
        &config.origin_service_url,
        &internal_token,
    )?);
    let destination: Arc<dyn RemoteSolvent> = Arc::new(SolventClient::from_base_url(
        &config.destination_service_url,
        &internal_token,
    )?);
    let proxy = CrossChainProxy::new(origin, destination, sagas);
    let proxy = match config.cctp {
        Some(cctp) => {
            let iris: Arc<dyn CctpAttestation> =
                Arc::new(CircleIrisClient::from_base_url(&cctp.iris_url)?);
            let completion: Arc<dyn CctpCompletion> = Arc::new(CircleCctpCompletion::new(
                iris,
                cctp.source_domain,
                cctp.destination_domain,
                cctp.finality_threshold,
                cctp.destination_app,
            ));
            proxy.with_cctp(completion)
        }
        None => proxy,
    };
    let proxy = Arc::new(proxy);

    let coordinator = Arc::clone(&proxy);
    let interval = Duration::from_millis(config.poll_interval_ms.max(100));
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            let orders = match coordinator.recoverable().await {
                Ok(orders) => orders,
                Err(error) => {
                    warn!(error = %error, "cross-chain recovery scan failed");
                    continue;
                }
            };
            for order in orders {
                if let Err(error) = coordinator.advance(order.order_id).await {
                    warn!(order_id = %order.order_id, error = %error, "cross-chain step will retry");
                }
            }
        }
    });

    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    info!(addr = %config.bind_addr, "cross-chain proxy listening");
    axum::serve(
        listener,
        crosschain_proxy_router(proxy, Arc::new(SystemClock)),
    )
    .await?;
    Ok(())
}

fn default_poll_interval_ms() -> u64 {
    2_000
}
