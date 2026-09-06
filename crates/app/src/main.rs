//! Solvent server — the composition root. Wires the config, a live registry (recovered from the
//! durable log and kept current by a chain watcher), the ledger (publishing one caps snapshot net of
//! holds), a chain provider, and the HTTP adapter into a running axum server.

mod config;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use solvent_adapters::balances::AlloyBalancesOracle;
use solvent_adapters::chain::ChainHead;
use solvent_adapters::execution::{AquaSettlementReader, SqliteFillStore, WalletkitExecutor};
use solvent_adapters::http::state::AppState;
use solvent_adapters::http::{self};
use solvent_adapters::ingest::uniswapx::{ServerCosigner, UniswapXFillBuilder};
use solvent_adapters::ledger::{AlloyBudgetSource, SqliteLedgerStore, SystemClock};
use solvent_adapters::registry::{AlloyChainSource, SqliteStore};
use solvent_adapters::routing::{BinanceFeed, GasPoller, MarketCache};
use solvent_adapters::trade::SqliteTradeStore;
use solvent_core::asset::AssetManager;
use solvent_core::balances::BalancesService;
use solvent_core::deps::balances::BalancesOracle;
use solvent_core::deps::ingest::FillBuilder;
use solvent_core::deps::ledger::BudgetSource;
use solvent_core::deps::registry::EventStore;
use solvent_core::deps::routing::{GasPrice, PriceOracle};
use solvent_core::deps::trade::TradeStore;
use solvent_core::execution::ExecutionService;
use solvent_core::ledger::LedgerService;
use solvent_core::pool::{DepthService, PoolService};
use solvent_core::primitives::routing::RoutingConfig;
use solvent_core::primitives::{ChainConfig, ChainId};
use solvent_core::quote::QuoteService;
use solvent_core::reconcile::ReconcileService;
use solvent_core::registry::{RegistrySync, SharedSnapshot};
use solvent_core::routing::LegCostResolver;
use solvent_core::swap::{SwapConfig, SwapService};
use solvent_core::SolventError;
use sqlx::SqlitePool;
use walletkit::adapters::policy::{AllowAll, DefaultPolicyEngine};
use walletkit::adapters::{LocalSigner, RedbStateStore, Transport};
use walletkit::core::deps::SubmissionOpts;
use walletkit::Wallet;

use crate::config::{load_token_list, Config, StartupError};

/// How often the background poller refreshes the cached chain head.
const BLOCK_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// How often the registry watcher scans the chain up to the current head.
const REGISTRY_SYNC_INTERVAL: Duration = Duration::from_secs(4);
/// How often the ledger re-reads every active maker's caps (one batched call).
const BUDGET_POLL_INTERVAL: Duration = Duration::from_secs(12);
/// How often the gas-price poller refreshes the market cache (off the quote path).
const GAS_POLL_INTERVAL: Duration = Duration::from_secs(12);
/// How often to reconcile in-flight fills and sweep orphaned holds (~2 blocks).
const RECONCILE_INTERVAL: Duration = Duration::from_secs(4);
/// Registry scan window: re-scan the last N blocks each tick (the dedup cache absorbs the overlap),
/// at this nominal block time. Devnet-generous; tune per chain.
const SCAN_OVERLAP_BLOCKS: u64 = 25;
const BLOCK_TIME_SECS: u64 = 2;
/// Routing funnel + split caps for the quote path (gas units unused until M3 wires gas pricing).
const MAX_CANDIDATES: usize = 16;
const MAX_LEGS: usize = 4;

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

    // The live server needs a durable store to recover the registry and ledger from; one pool backs
    // both (migrations create every table).
    let db_url = config
        .database_url
        .as_deref()
        .ok_or(StartupError::MissingDatabase)?;
    let pool = SqlitePool::connect(db_url).await?;
    let registry_store = Arc::new(SqliteStore::new(pool.clone()));
    registry_store.migrate().await.map_err(SolventError::from)?;
    let registry_store: Arc<dyn EventStore> = registry_store;

    // Watcher loop: recover the snapshot from the durable log for immediate readiness, then keep it
    // current by scanning the chain up to the head each tick.
    let registry = Arc::new(SharedSnapshot::default());
    let chain_config = ChainConfig::new(
        ChainId(config.chain_id),
        0,
        SCAN_OVERLAP_BLOCKS,
        BLOCK_TIME_SECS,
    );
    let chain_source = Arc::new(AlloyChainSource::new(
        Arc::new(provider.clone()),
        config.aqua_address,
        config.app_address,
        None,
    ));
    let registry_sync = Arc::new(RegistrySync::new(
        &chain_config,
        chain_source,
        Arc::clone(&registry_store),
        Arc::clone(&registry),
    ));
    registry_sync.recover().await?;

    // The ledger: durable reservations plus the synced caps snapshot the read paths share. Sequenced
    // after the registry recovers (L6.1) so strategy virtuals are non-zero — rebuild holds, then
    // publish the first caps snapshot.
    let budget_source: Arc<dyn BudgetSource> = Arc::new(AlloyBudgetSource::new(
        provider.clone(),
        config.aqua_address,
        config.app_address,
        Arc::clone(&registry),
    ));
    let ledger = Arc::new(LedgerService::new(
        Arc::new(SqliteLedgerStore::new(pool.clone())),
        budget_source,
        Arc::new(SystemClock),
    ));
    let trade_store: Arc<dyn TradeStore> = Arc::new(SqliteTradeStore::new(pool.clone()));
    ledger.recover().await?;
    if let Err(e) = ledger.sync_budgets(&registry.load()).await {
        tracing::warn!(error = %e, "initial budget sync failed; caps are empty until the next tick");
    }

    let assets = Arc::new(AssetManager::new(
        load_token_list(&config.token_list)?,
        Arc::clone(&registry),
    ));
    let pools = Arc::new(PoolService::new(Arc::clone(&registry), Arc::clone(&assets)));
    // An arbitrary wallet's holdings can't be pre-synced, so the balances endpoint reads them on
    // demand, batched into one round-trip per request.
    let balances_oracle: Arc<dyn BalancesOracle> = Arc::new(AlloyBalancesOracle::new(
        provider.clone(),
        config.aqua_address,
    ));
    let balances = Arc::new(BalancesService::new(balances_oracle, Arc::clone(&assets)));
    let depth = Arc::new(DepthService::new(
        Arc::clone(&registry),
        Arc::clone(&ledger),
        Arc::clone(&assets),
    ));
    // Market data for the per-leg gas cost: a cache the quote path reads lock-free (no RPC), kept
    // fresh by a gas poller (RPC) and the Binance price feed (WS). Both self-heal.
    let market = MarketCache::new();
    tokio::spawn(GasPoller::new(provider.clone(), Arc::clone(&market), GAS_POLL_INTERVAL).run());
    tokio::spawn(
        BinanceFeed::new(
            Arc::clone(&market),
            config.binance_ws_url.clone(),
            config.price_feed_symbols(),
        )
        .run(),
    );
    let gas: Arc<dyn GasPrice> = market.clone();
    let oracle: Arc<dyn PriceOracle> = market;
    // One per-leg gas resolver shared by both routing paths — quote and swap price gas the same way.
    let leg_cost = Arc::new(LegCostResolver::new(
        gas,
        oracle,
        Arc::clone(&assets),
        config.native_token,
        config.gas_units_per_leg,
    ));
    let quote = Arc::new(QuoteService::new(
        Arc::clone(&registry),
        Arc::clone(&ledger),
        Arc::clone(&assets),
        RoutingConfig::new(MAX_CANDIDATES, MAX_LEGS, config.gas_units_per_leg),
        Arc::new(SystemClock),
        Arc::clone(&leg_cost),
    ));

    // The swap write path. Signing keys are read from the environment — never the config file or a
    // log — so the resolver never persists them.
    let cosigner_key = std::env::var("SOLVENT_COSIGNER_KEY")
        .map_err(|_| StartupError::MissingSecret("SOLVENT_COSIGNER_KEY"))?;
    let filler_key = std::env::var("SOLVENT_SIGNER_KEY")
        .map_err(|_| StartupError::MissingSecret("SOLVENT_SIGNER_KEY"))?;
    let cosigner_signer: PrivateKeySigner = cosigner_key
        .parse()
        .map_err(|_| StartupError::Key("SOLVENT_COSIGNER_KEY is not a valid private key".into()))?;
    let cosigner = Arc::new(ServerCosigner::new(
        config.permit2,
        config.chain_id,
        cosigner_signer,
        config.filler,
        config.decay_window_secs,
    ));
    // The account the fill tx is signed and authorized by (the filler's owner).
    let filler_owner = filler_key
        .parse::<PrivateKeySigner>()
        .map_err(|_| StartupError::Key("SOLVENT_SIGNER_KEY is not a valid private key".into()))?
        .address();
    let filler_signer = LocalSigner::from_private_key(&filler_key)
        .map_err(|_| StartupError::Key("SOLVENT_SIGNER_KEY is not a valid private key".into()))?;
    let policy = DefaultPolicyEngine::new(
        vec![Box::new(AllowAll)],
        Arc::new(walletkit::adapters::SystemClock),
    );
    let transport = Transport::url(
        config
            .rpc_url
            .parse()
            .map_err(|e| StartupError::RpcUrl(format!("{e}")))?,
    )
    .map_err(|e| StartupError::RpcUrl(format!("{e}")))?;
    let wallet_state = RedbStateStore::open(&config.wallet_state_db)
        .map_err(|e| StartupError::WalletStore(e.to_string()))?;
    let wallet = Wallet::builder(
        Arc::new(transport),
        Arc::new(filler_signer),
        Arc::new(policy),
    )
    .store(Arc::new(wallet_state))
    .confirmations(config.confirmations)
    .bump_timeout(0)
    .build();
    let fill_store = Arc::new(SqliteFillStore::new(pool.clone()));
    let executor = Arc::new(WalletkitExecutor::new(
        wallet,
        SubmissionOpts::public(),
        fill_store,
    ));
    let settlement = Arc::new(AquaSettlementReader::new(
        Arc::new(provider.clone()),
        config.aqua_address,
    ));
    let execution = Arc::new(ExecutionService::new(
        executor.clone(),
        executor,
        settlement,
        Arc::clone(&ledger),
    ));
    let fill_builder: Arc<dyn FillBuilder> = Arc::new(UniswapXFillBuilder::new(config.app_address));
    let reconcile = Arc::new(ReconcileService::new(
        Arc::clone(&execution),
        Arc::clone(&trade_store),
        Arc::clone(&ledger),
        Arc::new(SystemClock),
    ));
    let swap = Arc::new(SwapService::new(
        Arc::clone(&registry),
        Arc::clone(&ledger),
        Arc::clone(&trade_store),
        Arc::clone(&execution),
        fill_builder,
        Arc::clone(&leg_cost),
        Arc::new(SystemClock),
        SwapConfig {
            routing: RoutingConfig::new(MAX_CANDIDATES, MAX_LEGS, config.gas_units_per_leg),
            chain_id: config.chain_id,
            filler: config.filler,
            filler_owner,
            reservation_ttl_secs: config.reservation_ttl_secs,
        },
    ));

    // Keep the registry live and the caps fresh — both supervised so a transient failure restarts.
    // Each closure owns its handles and re-clones them per restart; nothing below needs them again.
    let sync_head = head.clone();
    tokio::spawn(supervise("registry-sync", move || {
        run_registry_sync(
            Arc::clone(&registry_sync),
            sync_head.clone(),
            REGISTRY_SYNC_INTERVAL,
        )
    }));
    let ledger_registry = Arc::clone(&registry);
    tokio::spawn(supervise("ledger-sync", move || {
        run_ledger_sync(
            Arc::clone(&ledger),
            Arc::clone(&ledger_registry),
            BUDGET_POLL_INTERVAL,
        )
    }));
    tokio::spawn(supervise("reconcile", move || {
        run_reconcile(Arc::clone(&reconcile), RECONCILE_INTERVAL)
    }));

    let state = AppState {
        config: Arc::new(config.app_config()),
        head,
        assets,
        pools,
        depth,
        balances,
        quote,
        swap,
        cosigner,
        trades: trade_store,
        registry: Arc::clone(&registry),
        registry_store,
    };

    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(addr = %config.bind_addr, "solvent listening");
    axum::serve(listener, http::router(state))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
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

/// Scan the chain up to the latest head on an interval, folding new events into the live snapshot; a
/// failed tick keeps the last-good snapshot and retries. Skips until the head poller has a block.
async fn run_registry_sync(sync: Arc<RegistrySync>, head: ChainHead, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;
        let to_block = head.latest();
        if to_block == 0 {
            continue;
        }
        if let Err(e) = sync.sync_once(to_block).await {
            tracing::warn!(error = %e, "registry sync failed; retrying next tick");
        }
    }
}

/// Re-sync the ledger's caps on an interval; a failed tick keeps the last-good caps and retries.
async fn run_ledger_sync(
    ledger: Arc<LedgerService>,
    registry: Arc<SharedSnapshot>,
    interval: Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await; // the immediate first tick — the boot already ran one sync
    loop {
        ticker.tick().await;
        if let Err(e) = ledger.sync_budgets(&registry.load()).await {
            tracing::warn!(error = %e, "budget sync failed; keeping last-good caps");
        }
    }
}

/// Drive in-flight fills to settlement and sweep orphaned holds on an interval; a failed tick is
/// logged and retried next tick.
async fn run_reconcile(reconcile: Arc<ReconcileService>, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;
        match reconcile.tick().await {
            Ok(report) if report.settled > 0 || report.swept > 0 => {
                tracing::info!(
                    settled = report.settled,
                    swept = report.swept,
                    "reconcile tick"
                );
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "reconcile tick failed; retrying next tick"),
        }
    }
}

/// Resolve when the process is asked to stop (Ctrl-C), so in-flight requests can drain.
async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
