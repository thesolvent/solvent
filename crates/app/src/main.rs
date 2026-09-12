//! Solvent server — the composition root. Wires the config, a live registry (recovered from the
//! durable log and kept current by a chain watcher), the ledger (publishing one caps snapshot net of
//! holds), a chain provider, and the HTTP adapter into a running axum server.

mod config;

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::Address;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use solvent_adapters::balances::AlloyBalancesOracle;
use solvent_adapters::chain::ChainHead;
use solvent_adapters::crosschain::{
    AlloyDirectPlanAuthor, AlloyStepValidator, CcipStepMaterializer, DirectPlanAuthorConfig,
    ServiceLegQuoter, SqlitePreparationStore, SqliteStepStore,
};
use solvent_adapters::execution::{
    AquaSettlementReader, LocalPolicySigner, SqliteFillStore, WalletkitExecutor,
};
use solvent_adapters::http::state::AppState;
use solvent_adapters::http::{self};
use solvent_adapters::ingest::erc7683::{Erc7683FillBuilder, Erc7683Normalizer};
use solvent_adapters::ingest::oneinch::{
    OneInchApiClient, OneInchFeed, OneInchFillBuilder, OneInchNormalizer,
};
use solvent_adapters::ingest::uniswapx::{
    FeedHealth, HostedFeed, OrdersApiClient, Scope, ServerCosigner, UniswapXFillBuilder,
    UniswapXV2Normalizer,
};
use solvent_adapters::ingest::ProtocolFillBuilder;
use solvent_adapters::ledger::{AlloyBudgetSource, SqliteLedgerStore, SystemClock};
use solvent_adapters::metrics::{SqliteMakerMetrics, SqliteOrderLog, SqliteQuoteLog};
use solvent_adapters::rebate::{
    AlloyRebateChainSource, FillerRebateCallBuilder, SqliteRebateAccrualSource, SqliteRebateStore,
};
use solvent_adapters::registry::{AlloyBlockTimes, AlloyChainSource, SqliteStore};
use solvent_adapters::routing::{BinanceFeed, BinanceHistory, GasPoller, MarketCache};
use solvent_adapters::trade::SqliteTradeStore;
use solvent_core::asset::{AssetManager, PairHistoryService};
use solvent_core::balances::BalancesService;
use solvent_core::crosschain::{LocalCrossChainService, LocalStepService};
use solvent_core::decision::{DecisionConfig, DecisionDeps, DecisionService};
use solvent_core::deps::asset::PairPriceHistorySource;
use solvent_core::deps::balances::BalancesOracle;
use solvent_core::deps::crosschain::{
    DirectPlanAuthor, LegQuoter, PreparationStore, StepMaterializer, StepStore, StepValidator,
};
use solvent_core::deps::execution::{Execution, ExecutionAuthorizer, SimGate};
use solvent_core::deps::ingest::FillBuilder;
use solvent_core::deps::ingest::{Normalizer, OrderFeed};
use solvent_core::deps::ledger::BudgetSource;
use solvent_core::deps::maker_metrics::MakerMetricsStore;
use solvent_core::deps::order_log::OrderLog;
use solvent_core::deps::quote_log::QuoteLog;
use solvent_core::deps::rebate::{
    RebateAccrualSource, RebateChainSource, RebateMarketBook, RebateStore,
};
use solvent_core::deps::registry::EventStore;
use solvent_core::deps::routing::{GasPrice, PriceOracle};
use solvent_core::deps::trade::TradeStore;
use solvent_core::execution::ExecutionService;
use solvent_core::ingest::{Admission, IngestPipeline};
use solvent_core::ledger::LedgerService;
use solvent_core::maker::MakerService;
use solvent_core::obs::{error as obs_error, info, warn};
use solvent_core::pool::{DepthService, PoolService};
use solvent_core::primitives::crosschain::RemoteCommand;
use solvent_core::primitives::ingest::{ExecutionFeePolicy, Intent, ProtocolId};
use solvent_core::primitives::routing::RoutingConfig;
use solvent_core::primitives::{ChainConfig, ChainId};
use solvent_core::quote::QuoteService;
use solvent_core::rebate::{
    RebateMarketData, RebatePolicy, RebatePolicyConfig, RebateService, RebateServiceConfig,
    RebateWorker, RebateWorkerConfig,
};
use solvent_core::reconcile::ReconcileService;
use solvent_core::registry::{RegistrySync, SharedSnapshot};
use solvent_core::routing::{LegCostResolver, StrategyGuard};
use solvent_core::swap::{SwapConfig, SwapService};
use solvent_core::trade::TradeService;
use solvent_core::valuation::Valuation;
use solvent_core::SolventError;
use sqlx::SqlitePool;
use walletkit::adapters::policy::{AllowAll, DefaultPolicyEngine};
use walletkit::adapters::{LocalSigner, RedbStateStore, Transport};
use walletkit::core::deps::SubmissionOpts;
use walletkit::Wallet;

use crate::config::{load_token_list, Config, StartupError};

sol! {
    #[sol(rpc)]
    interface FillerConfiguration {
        function TAKER_CREDENTIAL() external view returns (address);
    }
}

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
/// Routing funnel + split caps for the quote path (gas units unused until gas pricing is wired).
const MAX_CANDIDATES: usize = 16;
const MAX_LEGS: usize = 4;
/// Backpressure between ingest and the decision loop. A full channel slows the feed rather than
/// dropping orders.
const INTENT_CHANNEL_CAPACITY: usize = 256;
/// How often held orders are re-priced — one block, since that is the rate at which the state a
/// decision rests on can change.
const DECISION_TICK: Duration = Duration::from_secs(12);

#[tokio::main]
async fn main() -> Result<(), StartupError> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config_path = std::env::var("SOLVENT_CONFIG").unwrap_or_else(|_| "solvent".to_string());
    let config = Config::load(&config_path)?;
    let erc7683_contracts = config.erc7683_contracts()?;

    let rpc = config
        .rpc_url
        .parse()
        .map_err(|e| StartupError::RpcUrl(format!("{e}")))?;
    let provider = ProviderBuilder::new().connect_http(rpc).erased();
    let taker_credential = FillerConfiguration::new(config.filler, provider.clone())
        .TAKER_CREDENTIAL()
        .call()
        .await
        .map_err(|error| StartupError::FillerConfiguration(error.to_string()))?;
    if let Some(contracts) = erc7683_contracts {
        let erc7683_credential = FillerConfiguration::new(contracts.filler, provider.clone())
            .TAKER_CREDENTIAL()
            .call()
            .await
            .map_err(|error| StartupError::FillerConfiguration(error.to_string()))?;
        if erc7683_credential != taker_credential {
            return Err(StartupError::FillerConfiguration(
                "UniswapX and ERC-7683 fillers must share one taker credential".to_string(),
            ));
        }
    }
    let erc7683_fee_policy = erc7683_contracts
        .map(|_| ExecutionFeePolicy::new(config.erc7683_executor_fee_bps))
        .transpose()
        .map_err(StartupError::from)?;
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
        config.registry_start_block,
        SCAN_OVERLAP_BLOCKS,
        BLOCK_TIME_SECS,
    );
    let chain_source = Arc::new(AlloyChainSource::new(
        Arc::new(provider.clone()),
        config.aqua_address,
        config.app_address,
        config.registry_scan_span,
    ));
    let registry_sync = Arc::new(RegistrySync::new(
        &chain_config,
        chain_source,
        Arc::clone(&registry_store),
        Arc::clone(&registry),
    ));
    registry_sync.recover().await?;

    // The ledger: durable reservations plus the synced caps snapshot the read paths share. Sequenced
    // after the registry recovers so strategy virtuals are non-zero — rebuild holds, then publish
    // the first caps snapshot.
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
    let order_log = Arc::new(SqliteOrderLog::new(pool.clone()));
    let quote_log: Arc<dyn QuoteLog> =
        Arc::new(SqliteQuoteLog::new(pool.clone(), Arc::new(SystemClock)));
    ledger.recover().await?;
    if let Err(e) = ledger.sync_budgets(&registry.load()).await {
        tracing::warn!(error = %e, "initial budget sync failed; caps are empty until the next tick");
    }

    let assets = Arc::new(AssetManager::new(
        load_token_list(&config.token_list)?,
        Arc::clone(&registry),
    ));
    // Market data: a cache the quote path reads lock-free (no RPC), kept fresh by a gas poller (RPC)
    // and the Binance price feed (WS). Both self-heal. It also backs USD valuation across the reads.
    let market = MarketCache::new();
    for token in &config.usd_stable_pegs {
        market.seed_peg(*token);
    }
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
    let oracle: Arc<dyn PriceOracle> = market.clone();
    let rebate_market: Arc<dyn RebateMarketBook> = market;
    let valuation = Arc::new(Valuation::new(Arc::clone(&oracle)));
    let history_source: Arc<dyn PairPriceHistorySource> = Arc::new(
        BinanceHistory::new(
            config.binance_rest_url.clone(),
            config.price_feed_symbols(),
            &config.usd_stable_pegs,
        )
        .map_err(SolventError::from)?,
    );
    let pair_history = Arc::new(PairHistoryService::new(history_source));
    // One per-leg gas resolver shared by both routing paths — quote and swap price gas the same way.
    let leg_cost = Arc::new(LegCostResolver::new(
        Arc::clone(&gas),
        oracle,
        Arc::clone(&assets),
        config.native_token,
        config.gas_units_per_leg,
    ));

    let maker_metrics: Arc<dyn MakerMetricsStore> = Arc::new(SqliteMakerMetrics::new(pool.clone()));

    // An arbitrary wallet's holdings can't be pre-synced, so callers read them on demand, batched
    // into one round-trip per wallet.
    let balances_oracle: Arc<dyn BalancesOracle> = Arc::new(AlloyBalancesOracle::new(
        provider.clone(),
        config.aqua_address,
    ));

    let pools = Arc::new(PoolService::new(
        Arc::clone(&registry),
        Arc::clone(&assets),
        Arc::clone(&valuation),
        Arc::clone(&maker_metrics),
        Arc::clone(&balances_oracle),
        Arc::new(SystemClock),
    ));
    let balances = Arc::new(BalancesService::new(
        Arc::clone(&balances_oracle),
        Arc::clone(&assets),
        Arc::clone(&valuation),
    ));
    let makers = Arc::new(MakerService::new(
        Arc::clone(&registry),
        Arc::clone(&assets),
        Arc::clone(&valuation),
        maker_metrics,
        Arc::clone(&balances_oracle),
        Arc::clone(&registry_store),
        Arc::new(AlloyBlockTimes::new(Arc::new(provider.clone()))),
        Arc::new(SystemClock),
        ChainId(config.chain_id),
    ));
    let strategy_guard = Arc::new(StrategyGuard::default());
    let depth = Arc::new(DepthService::new(
        Arc::clone(&registry),
        Arc::clone(&ledger),
        Arc::clone(&strategy_guard),
        Arc::clone(&assets),
    ));
    let quote = Arc::new(QuoteService::new(
        Arc::clone(&registry),
        Arc::clone(&ledger),
        Arc::clone(&strategy_guard),
        Arc::clone(&assets),
        RoutingConfig::new(MAX_CANDIDATES, MAX_LEGS, config.gas_units_per_leg),
        Arc::new(SystemClock),
        Arc::clone(&leg_cost),
        Arc::clone(&valuation),
    ));

    // The swap write path. Signing keys are read from the environment — never the config file or a
    // log — so the resolver never persists them.
    let cosigner_key = std::env::var("SOLVENT_COSIGNER_KEY")
        .map_err(|_| StartupError::MissingSecret("SOLVENT_COSIGNER_KEY"))?;
    let filler_key = std::env::var("SOLVENT_SIGNER_KEY")
        .map_err(|_| StartupError::MissingSecret("SOLVENT_SIGNER_KEY"))?;
    let policy_signer_key = std::env::var("SOLVENT_POLICY_SIGNER_KEY")
        .map_err(|_| StartupError::MissingSecret("SOLVENT_POLICY_SIGNER_KEY"))?;
    let cosigner_signer: PrivateKeySigner = cosigner_key
        .parse()
        .map_err(|_| StartupError::Key("SOLVENT_COSIGNER_KEY is not a valid private key".into()))?;
    let cosigner = Arc::new(ServerCosigner::new(
        config.permit2,
        config.reactor,
        config.chain_id,
        cosigner_signer,
        config.filler,
        config.decay_window_secs,
    ));
    // The self-venue path cosigns with our own key, so that is the identity its normalizer pins.
    // The order feed's normalizer pins Uniswap's cosigner instead.
    let normalizer = Arc::new(UniswapXV2Normalizer::new(
        config.reactor,
        vec![cosigner.address()],
    ));

    // The account the fill tx is signed and authorized by (the filler's owner).
    let filler_owner = filler_key
        .parse::<PrivateKeySigner>()
        .map_err(|_| StartupError::Key("SOLVENT_SIGNER_KEY is not a valid private key".into()))?
        .address();
    let filler_signer = LocalSigner::from_private_key(&filler_key)
        .map_err(|_| StartupError::Key("SOLVENT_SIGNER_KEY is not a valid private key".into()))?;
    let policy_signer = policy_signer_key.parse::<PrivateKeySigner>().map_err(|_| {
        StartupError::Key("SOLVENT_POLICY_SIGNER_KEY is not a valid private key".into())
    })?;
    let uniswapx_authorizer: Arc<dyn ExecutionAuthorizer> = Arc::new(LocalPolicySigner::new(
        config.chain_id,
        config.filler,
        policy_signer.clone(),
    ));
    let erc7683_authorizer = erc7683_contracts.map(|contracts| {
        Arc::new(LocalPolicySigner::new(
            config.chain_id,
            contracts.filler,
            policy_signer,
        )) as Arc<dyn ExecutionAuthorizer>
    });
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
        executor.clone(),
        settlement,
        Arc::clone(&ledger),
    ));
    if let Some(crosschain) = &config.crosschain {
        let internal_token = std::env::var("SOLVENT_INTERNAL_TOKEN")
            .map_err(|_| StartupError::MissingSecret("SOLVENT_INTERNAL_TOKEN"))?;
        let mut authorization = format!("Bearer {internal_token}")
            .parse::<axum::http::HeaderValue>()
            .map_err(|_| {
                StartupError::Key("SOLVENT_INTERNAL_TOKEN is not a valid HTTP header".into())
            })?;
        authorization.set_sensitive(true);
        let preparations: Arc<dyn PreparationStore> =
            Arc::new(SqlitePreparationStore::new(pool.clone()));
        let crosschain_quotes: Arc<dyn solvent_core::deps::crosschain::LegQuoteStore> = Arc::new(
            solvent_adapters::crosschain::SqliteLegQuoteStore::new(pool.clone()),
        );
        let local_quoter: Arc<dyn LegQuoter> = Arc::new(ServiceLegQuoter::new(
            ChainId(config.chain_id),
            Arc::clone(&quote),
            head.clone(),
            Arc::new(SystemClock),
            crosschain.quote_ttl_secs,
        ));
        let local = Arc::new(LocalCrossChainService::new(
            ChainId(config.chain_id),
            local_quoter,
            crosschain_quotes,
            preparations,
            Arc::clone(&ledger),
            Arc::new(SystemClock),
        ));
        let mut targets = BTreeMap::new();
        if !crosschain.destination_app.is_zero() {
            targets.insert(RemoteCommand::Deliver, crosschain.destination_app);
            targets.insert(RemoteCommand::CloseDestination, crosschain.destination_app);
        }
        if !crosschain.origin_settler.is_zero() {
            targets.insert(RemoteCommand::ClaimOrigin, crosschain.origin_settler);
        }
        if !crosschain.proof_outbox.is_zero() {
            targets.insert(RemoteCommand::DispatchFillProof, crosschain.proof_outbox);
            targets.insert(RemoteCommand::DispatchRepayment, crosschain.proof_outbox);
        }
        let step_store: Arc<dyn StepStore> = Arc::new(SqliteStepStore::new(pool.clone()));
        let execution_port: Arc<dyn Execution> = executor.clone();
        let simulator: Arc<dyn SimGate> = executor.clone();
        let materializer: Arc<dyn StepMaterializer> =
            Arc::new(CcipStepMaterializer::new(provider.clone()));
        let validator: Arc<dyn StepValidator> = Arc::new(
            AlloyStepValidator::new(filler_owner, config.native_token)
                .with_applications(crosschain.origin_settler, crosschain.destination_app),
        );
        let steps = Arc::new(
            LocalStepService::new(
                ChainId(config.chain_id),
                filler_owner,
                targets,
                step_store,
                execution_port,
                simulator,
            )
            .with_validator(validator)
            .with_materializer(materializer),
        );
        let direct_author: Option<Arc<dyn DirectPlanAuthor>> = crosschain
            .direct_author
            .as_ref()
            .map(|direct| {
                let maker_key = std::env::var("SOLVENT_CROSSCHAIN_MAKER_KEY")
                    .map_err(|_| StartupError::MissingSecret("SOLVENT_CROSSCHAIN_MAKER_KEY"))?;
                let maker = maker_key
                    .parse::<PrivateKeySigner>()
                    .map_err(|error| StartupError::Key(error.to_string()))?;
                AlloyDirectPlanAuthor::new(
                    DirectPlanAuthorConfig {
                        origin_chain: ChainId(direct.origin_chain_id),
                        destination_chain: ChainId(config.chain_id),
                        origin_settler: crosschain.origin_settler,
                        destination_app: crosschain.destination_app,
                        origin_proof_outbox: direct.origin_proof_outbox,
                        destination_proof_outbox: direct.destination_proof_outbox,
                        origin_strategy_hash: solvent_core::primitives::StrategyHash(
                            direct.origin_strategy_hash,
                        ),
                    },
                    maker,
                )
                .map(|author| Arc::new(author) as Arc<dyn DirectPlanAuthor>)
                .map_err(SolventError::from)
                .map_err(StartupError::from)
            })
            .transpose()?;
        let internal_listener = tokio::net::TcpListener::bind(crosschain.bind_addr).await?;
        let internal_router = http::crosschain_internal_router_with_author(
            local,
            steps,
            authorization,
            direct_author,
        );
        info!(addr = %crosschain.bind_addr, "private cross-chain service listening");
        tokio::spawn(async move {
            if let Err(error) = axum::serve(internal_listener, internal_router).await {
                obs_error!(error = %error, "private cross-chain service stopped");
            }
        });
    }
    let rebate_store: Arc<dyn RebateStore> = Arc::new(SqliteRebateStore::new(pool.clone()));
    let rebates = Arc::new(RebateService::new(
        RebatePolicy::new(RebatePolicyConfig::new(
            config.rebate.deviation_threshold_bps,
            config.rebate.gas_safety_bps,
        )),
        Arc::clone(&ledger),
        Arc::clone(&strategy_guard),
        Arc::clone(&rebate_store),
        Arc::new(FillerRebateCallBuilder::new(
            taker_credential,
            Arc::clone(&uniswapx_authorizer),
        )),
        RebateMarketData::new(rebate_market, gas, Arc::clone(&assets)),
        RebateServiceConfig::new(
            config.native_token,
            config.rebate.gas_units,
            config.rebate.market_max_age_secs,
        ),
    ));
    rebates.recover().await?;
    let rebate_accruals: Arc<dyn RebateAccrualSource> = Arc::new(SqliteRebateAccrualSource::new(
        pool.clone(),
        config.app_address,
    ));
    let rebate_chain: Arc<dyn RebateChainSource> = Arc::new(AlloyRebateChainSource::new(
        Arc::new(provider.clone()),
        config.filler,
        None,
    ));
    let rebate_worker = Arc::new(RebateWorker::new(
        Arc::clone(&rebates),
        rebate_accruals,
        rebate_chain,
        rebate_store,
        Arc::clone(&registry_store),
        Arc::clone(&registry),
        Arc::clone(&ledger),
        Arc::new(SystemClock),
        RebateWorkerConfig::new(
            ChainId(config.chain_id),
            config.rebate.start_block,
            config.rebate.authorization_ttl_blocks,
            config.reservation_ttl_secs,
        ),
    ));
    // Each protocol names its own target contract in the `PreparedFill` it returns (`SwapService`
    // no longer assumes a single filler address). 1inch's `oneinch_filler` has no real mainnet
    // deployment yet: an unconfigured (zero) address still has no code, so a simulated call against
    // it returns success with empty data rather than reverting, which would otherwise read as a
    // real fillable order and broadcast a pointless, gas-spending transaction. Leaving the builder
    // out entirely when unset makes that protocol decline the same clean way an unregistered one
    // already does, instead of silently misreporting as submitted.
    let uniswapx_fill: Arc<dyn FillBuilder> = Arc::new(UniswapXFillBuilder::new(
        config.app_address,
        config.filler,
        taker_credential,
        uniswapx_authorizer,
    ));
    let erc7683_fill = erc7683_contracts
        .zip(erc7683_authorizer)
        .map(|(contracts, authorizer)| {
            Arc::new(Erc7683FillBuilder::new(
                config.app_address,
                contracts.settler,
                contracts.filler,
                filler_owner,
                taker_credential,
                authorizer,
            )) as Arc<dyn FillBuilder>
        });
    let oneinch_fill = (config.oneinch_filler != Address::ZERO).then(|| {
        Arc::new(OneInchFillBuilder::new(
            config.app_address,
            config.oneinch_filler,
        )) as Arc<dyn FillBuilder>
    });
    let fill_builder: Arc<dyn FillBuilder> = Arc::new(ProtocolFillBuilder::new(
        uniswapx_fill,
        erc7683_fill,
        oneinch_fill,
    ));
    let erc7683 = erc7683_contracts
        .zip(erc7683_fee_policy)
        .map(|(contracts, fee_policy)| {
            Arc::new(Erc7683Normalizer::new(
                ChainId(config.chain_id),
                config.permit2,
                contracts.settler,
                fee_policy,
            ))
        });
    let reconcile = Arc::new(ReconcileService::new(
        Arc::clone(&execution),
        Arc::clone(&trade_store),
        Arc::clone(&ledger),
        Arc::new(SystemClock),
    ));
    let swap = Arc::new(SwapService::new(
        Arc::clone(&registry),
        Arc::clone(&ledger),
        Arc::clone(&strategy_guard),
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
    // Kept before the supervisor takes ownership, for the decision loop wired further down.
    let decision_ledger = Arc::clone(&ledger);
    let ledger_registry = Arc::clone(&registry);
    tokio::spawn(supervise("ledger-sync", move || {
        run_ledger_sync(
            Arc::clone(&ledger),
            Arc::clone(&ledger_registry),
            BUDGET_POLL_INTERVAL,
        )
    }));
    let reconcile_head = head.clone();
    tokio::spawn(supervise("reconcile", move || {
        run_reconcile(
            Arc::clone(&reconcile),
            Arc::clone(&rebate_worker),
            reconcile_head.clone(),
            RECONCILE_INTERVAL,
        )
    }));

    // The live order feed(s). Orders are off-chain messages until someone fills them, so polling is
    // the only way to see one. Left off entirely when neither endpoint is configured, in which case
    // the resolver takes orders solely from its own submit path.
    let mut feed_health_handle: Option<Arc<FeedHealth>> = None;
    if config.orders_api_url.is_some() || config.oneinch_orderbook_url.is_some() {
        // Fall back to the token list: those are the assets the registry can price, so nothing
        // outside them is fillable anyway.
        let admitted: BTreeSet<Address> = match config.admitted_tokens.is_empty() {
            true => assets
                .catalog_tokens()
                .into_iter()
                .map(|token| token.address)
                .collect(),
            false => config.admitted_tokens.iter().copied().collect(),
        };
        let mut normalizers: BTreeMap<ProtocolId, Arc<dyn Normalizer>> = BTreeMap::new();
        let mut feeds: Vec<Arc<dyn OrderFeed>> = Vec::new();

        if let Some(orders_api_url) = config.orders_api_url.clone() {
            let feed_normalizer: Arc<dyn Normalizer> = Arc::new(UniswapXV2Normalizer::new(
                config.reactor,
                config.expected_cosigners.clone(),
            ));
            if config.expected_cosigners.is_empty() {
                tracing::warn!(
                    "no expected_cosigners configured; the order feed will admit nothing"
                );
            }
            normalizers.insert(ProtocolId::UniswapXV2, feed_normalizer);
            let orders_client = Arc::new(
                OrdersApiClient::new(
                    orders_api_url,
                    ChainId(config.chain_id),
                    config.order_type.clone(),
                )
                .map_err(|e| StartupError::OrdersFeed(e.to_string()))?,
            );
            let feed_health = Arc::new(FeedHealth::new(Duration::from_secs(
                config.feed_silence_secs,
            )));
            feed_health_handle = Some(Arc::clone(&feed_health));
            // Orders already assigned to us are polled apart from the wider book: the exclusivity
            // window is seconds long and discovery must not queue behind a page of everything else.
            let feed: Arc<dyn OrderFeed> = Arc::new(HostedFeed::new(
                orders_client,
                ChainId(config.chain_id),
                vec![Scope::ExclusiveTo(config.filler), Scope::Book],
                Duration::from_millis(config.order_poll_ms),
                Arc::clone(&feed_health),
            ));
            feeds.push(feed);
        }

        if let Some(oneinch_url) = config.oneinch_orderbook_url.clone() {
            let oneinch_key = std::env::var("ONEINCH_API_KEY")
                .map_err(|_| StartupError::MissingSecret("ONEINCH_API_KEY"))?;
            normalizers.insert(ProtocolId::OneInchLimitOrder, Arc::new(OneInchNormalizer));
            let oneinch_client = Arc::new(
                OneInchApiClient::new(oneinch_url, ChainId(config.chain_id), oneinch_key)
                    .map_err(|e| StartupError::OrdersFeed(e.to_string()))?,
            );
            let oneinch_health = Arc::new(FeedHealth::new(Duration::from_secs(
                config.feed_silence_secs,
            )));
            let oneinch_feed: Arc<dyn OrderFeed> = Arc::new(OneInchFeed::new(
                oneinch_client,
                ChainId(config.chain_id),
                Duration::from_millis(config.order_poll_ms),
                oneinch_health,
            ));
            feeds.push(oneinch_feed);
        }

        let pipeline = Arc::new(IngestPipeline::new(
            normalizers,
            Duration::from_secs(config.dedup_ttl_secs),
            config.dedup_capacity,
            Arc::new(SystemClock),
            Admission {
                supported_chains: BTreeSet::from([ChainId(config.chain_id)]),
                tokens: admitted,
                max_outputs: config.max_outputs,
            },
            Some(Arc::clone(&order_log) as Arc<dyn OrderLog>),
        ));

        let (tx, rx) = tokio::sync::mpsc::channel::<Intent>(INTENT_CHANNEL_CAPACITY);
        tokio::spawn(supervise("ingest", move || {
            let pipeline = Arc::clone(&pipeline);
            let feeds = feeds.clone();
            let tx = tx.clone();
            async move { pipeline.run(feeds, tx).await }
        }));

        // Re-price every held order once per block: that is how often the state a decision rests on
        // can change, and an order's price only moves with time.
        let decision = Arc::new(DecisionService::new(
            DecisionDeps {
                registry: Arc::clone(&registry),
                ledger: Arc::clone(&decision_ledger),
                swap: Arc::clone(&swap),
                leg_cost: Arc::clone(&leg_cost),
                valuation: Arc::clone(&valuation),
                clock: Arc::new(SystemClock),
                order_log: Some(Arc::clone(&order_log) as Arc<dyn OrderLog>),
            },
            DecisionConfig {
                routing: RoutingConfig::new(MAX_CANDIDATES, MAX_LEGS, config.gas_units_per_leg),
                filler: config.filler,
                max_tracked: config.max_tracked_intents,
            },
        ));
        // Not `supervise`d: the loop owns the receiving half of the intent channel, which cannot be
        // handed to a restart. What matters is that its ending is loud — otherwise the channel
        // fills, ingest blocks on a full send, and the process keeps polling while filling nothing.
        tokio::spawn(async move {
            let ticks = futures::stream::unfold((), |()| async {
                tokio::time::sleep(DECISION_TICK).await;
                Some(((), ()))
            });
            decision.run(rx, Box::pin(ticks)).await;
            tracing::error!("decision loop ended; no order will be filled until restart");
        });
        tracing::info!("order feed polling every {}ms", config.order_poll_ms);
    }

    let trades = Arc::new(TradeService::new(
        Arc::clone(&trade_store),
        Arc::clone(&assets),
        Arc::clone(&valuation),
        Arc::clone(&registry),
    ));
    let state = AppState {
        config: Arc::new(config.app_config(
            cosigner.address(),
            taker_credential,
            erc7683_contracts,
        )),
        head,
        assets,
        pair_history,
        pools,
        depth: solvent_adapters::http::DepthReader::new(depth),
        balances,
        makers,
        quote,
        swap,
        rebates,
        cosigner,
        normalizer,
        erc7683,
        erc7683_fee_policy,
        trades,
        registry: Arc::clone(&registry),
        registry_store,
        valuation,
        quote_log,
        feed_health: feed_health_handle,
        order_log: Some(Arc::clone(&order_log)),
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
async fn run_reconcile(
    reconcile: Arc<ReconcileService>,
    rebates: Arc<RebateWorker>,
    head: ChainHead,
    interval: Duration,
) {
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
        match rebates.tick(head.latest()).await {
            Ok(report)
                if report.accrued > 0
                    || report.evaluated > 0
                    || report.settled > 0
                    || report.expired > 0 =>
            {
                info!(
                    accrued = report.accrued,
                    evaluated = report.evaluated,
                    settled = report.settled,
                    expired = report.expired,
                    "rebate tick"
                );
            }
            Ok(_) => {}
            Err(e) => warn!(error = %e, "rebate tick failed; retrying next tick"),
        }
    }
}

/// Resolve when the process is asked to stop (Ctrl-C), so in-flight requests can drain.
async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
