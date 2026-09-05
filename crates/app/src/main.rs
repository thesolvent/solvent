//! Solvent server — the composition root. Wires the config, the registry snapshot (hydrated from the
//! durable event log), a chain provider, and the HTTP adapter into a running axum server.

mod config;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use alloy::providers::{Provider, ProviderBuilder};
use solvent_adapters::chain::ChainHead;
use solvent_adapters::http::state::AppState;
use solvent_adapters::http::{self};
use solvent_adapters::ledger::AlloyBudgetSource;
use solvent_adapters::registry::SqliteStore;
use solvent_core::asset::AssetManager;
use solvent_core::deps::ledger::BudgetSource;
use solvent_core::deps::registry::Store;
use solvent_core::ledger::BudgetCache;
use solvent_core::pool::{DepthService, PoolService};
use solvent_core::primitives::registry::Snapshot;
use solvent_core::primitives::ChainId;
use solvent_core::registry::SharedSnapshot;
use solvent_core::SolventError;
use sqlx::SqlitePool;

use crate::config::{load_token_list, Config, StartupError};

/// How often the background poller refreshes the cached chain head.
const BLOCK_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// How often the budget cache re-reads every active maker's pullable wallet (one batched call).
const BUDGET_POLL_INTERVAL: Duration = Duration::from_secs(12);

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
    let (head, poller) = ChainHead::new(provider.clone(), BLOCK_POLL_INTERVAL);
    tokio::spawn(poller);

    let registry =
        Arc::new(hydrate(config.database_url.as_deref(), ChainId(config.chain_id)).await?);
    let assets = Arc::new(AssetManager::new(
        load_token_list(&config.token_list)?,
        Arc::clone(&registry),
    ));
    let pools = Arc::new(PoolService::new(Arc::clone(&registry), Arc::clone(&assets)));
    // Every maker's executable cap, synced off the request path: the cache batch-reads all active
    // makers' pullable wallets each tick, and depth reads its caps from it lock-free — no per-request
    // RPC. (The quote path converges on one net-of-reservations snapshot in M2; see BudgetCache.)
    let source: Arc<dyn BudgetSource> = Arc::new(AlloyBudgetSource::new(
        provider,
        config.aqua_address,
        config.app_address,
        Arc::clone(&registry),
    ));
    let budgets = Arc::new(BudgetCache::new(source, Arc::clone(&registry)));
    if let Err(e) = budgets.refresh().await {
        tracing::warn!(error = %e, "initial budget sync failed; depth is empty until the next tick");
    }
    let sync_cache = Arc::clone(&budgets);
    tokio::spawn(supervise("budget-sync", move || {
        run_budget_sync(Arc::clone(&sync_cache), BUDGET_POLL_INTERVAL)
    }));
    let depth = Arc::new(DepthService::new(
        Arc::clone(&registry),
        budgets,
        Arc::clone(&assets),
    ));

    let state = AppState {
        config: Arc::new(config.app_config()),
        head,
        assets,
        pools,
        depth,
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

/// Aborts a task via its handle when dropped, so a supervisor that stops takes its child down with
/// it rather than leaking it.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Restart backoff bounds for a supervised task.
const SUPERVISE_MIN_BACKOFF: Duration = Duration::from_secs(1);
const SUPERVISE_MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Run a background task forever, restarting it on panic with capped exponential backoff. A clean
/// return restarts immediately (no backoff); a cancellation — the `JoinSet`/handle being aborted on
/// shutdown — stops. `make` builds a fresh task future for each run.
async fn supervise<F, Fut>(task: &'static str, mut make: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let mut backoff = SUPERVISE_MIN_BACKOFF;
    loop {
        let handle = tokio::spawn(make());
        let _abort = AbortOnDrop(handle.abort_handle());
        match handle.await {
            Ok(()) => {
                backoff = SUPERVISE_MIN_BACKOFF;
                continue;
            }
            Err(join) if join.is_cancelled() => return,
            Err(join) => tracing::error!(
                task,
                panic = join.is_panic(),
                backoff_secs = backoff.as_secs(),
                "background task died; restarting"
            ),
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(SUPERVISE_MAX_BACKOFF);
    }
}

/// Re-sync the budget cache on an interval; a failed tick keeps the last-good caps and retries.
async fn run_budget_sync(cache: Arc<BudgetCache>, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await; // the immediate first tick — the boot already ran one sync
    loop {
        ticker.tick().await;
        if let Err(e) = cache.refresh().await {
            tracing::warn!(error = %e, "budget sync failed; keeping last-good caps");
        }
    }
}

/// Resolve when the process is asked to stop (Ctrl-C), so in-flight requests can drain.
async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
