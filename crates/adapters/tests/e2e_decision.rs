//! The whole loop, end to end, with nothing hand-fed: an order is published to a stand-in for
//! Uniswap's Orders API, the production feed polls it, the pipeline validates it, and the decision
//! loop prices it every tick until it is worth filling — then fills it against the real contracts.
//!
//! What this proves that the earlier E2Es do not is the *waiting*. Those reserved and filled a plan
//! the test had already decided on. Here nobody decides but the loop: it must decline an order it
//! cannot yet afford, keep holding it, and act at the moment the price crosses. Chain time is moved
//! forward to make that moment arrive, so the test is deterministic rather than a race.
//!
//! Everything below the feed is the production path — same client, same normalizer, same admission,
//! same `DecisionService`, same `SwapService`. Only the base URL and the cosigner differ from
//! mainnet, and both are configuration. Gated on anvil.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::{address, Address, U256};
use alloy::providers::Provider;
use alloy::signers::local::PrivateKeySigner;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use common::{
    balance_of, first_supported, ledger, setup, synced, Harness, MockERC20, Stack, PERMIT2,
};
use solvent_adapters::execution::{AquaSettlementReader, SqliteFillStore, WalletkitExecutor};
use solvent_adapters::ingest::uniswapx::{
    FeedHealth, HostedFeed, OrderSpec, OrdersApiClient, Scope, SignedOrderBuilder,
    UniswapXV2Normalizer,
};
use solvent_adapters::routing::MarketCache;
use solvent_adapters::trade::SqliteTradeStore;
use solvent_core::asset::{AssetManager, TokenList, TokenMeta};
use solvent_core::decision::{DecisionConfig, DecisionDeps, DecisionService};
use solvent_core::deps::ingest::{FillBuilder, Normalizer, OrderFeed};
use solvent_core::deps::ledger::Clock;
use solvent_core::deps::routing::{GasPrice, PriceOracle};
use solvent_core::deps::trade::TradeStore;
use solvent_core::execution::ExecutionService;
use solvent_core::ingest::{Admission, IngestPipeline};
use solvent_core::ledger::LedgerService;
use solvent_core::primitives::ingest::{Intent, ProtocolId};
use solvent_core::primitives::routing::RoutingConfig;
use solvent_core::primitives::ChainId;
use solvent_core::reconcile::ReconcileService;
use solvent_core::routing::{LegCostResolver, StrategyGuard};
use solvent_core::swap::{SwapConfig, SwapService};
use solvent_core::valuation::Valuation;
use sqlx::SqlitePool;
use std::path::Path;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use walletkit::adapters::policy::{AllowAll, DefaultPolicyEngine};
use walletkit::adapters::{LocalSigner, RedbStateStore, Transport};
use walletkit::core::deps::SubmissionOpts;
use walletkit::Wallet;

const RECIPIENT: Address = address!("dEADbEEF00000000000000000000000000000000");
/// Whoever the auction gave the window to. Not us — so the toll applies until it lapses.
const ANOTHER_FILLER: Address = address!("5555555555555555555555555555555555555555");

// ---------------------------------------------------------------- the Orders API stand-in

/// Serves one order in the shape the real endpoint does, so the production client and feed run
/// against it unmodified.
async fn orders_page(State(page): State<Arc<Value>>) -> Json<Value> {
    Json((*page).clone())
}

async fn serve_orders(page: Value) -> String {
    let app = Router::new()
        .route("/orders", get(orders_page))
        .with_state(Arc::new(page));
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind mirror");
    let addr = listener.local_addr().expect("mirror addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

// ---------------------------------------------------------------- chain-anchored clock

/// Reads the chain's own clock. The loop prices an order at `now` and the settler re-prices it at
/// `block.timestamp`; if the two drift the fill reverts, so the test moves one clock and both
/// follow.
struct ChainClock(Arc<parking_lot::Mutex<u64>>);

impl Clock for ChainClock {
    fn now_unix(&self) -> u64 {
        *self.0.lock()
    }
}

async fn chain_now(h: &Harness) -> u64 {
    h.maker_provider
        .get_block_by_number(alloy::eips::BlockNumberOrTag::Latest)
        .await
        .expect("latest block")
        .expect("a block exists")
        .header
        .timestamp
}

/// Move chain time forward and mine, so the next block — and any `eth_call` against it — sees it.
async fn advance(h: &Harness, seconds: u64) {
    // anvil answers with the new timestamp, not unit.
    let _: u64 = h
        .maker_provider
        .raw_request("evm_increaseTime".into(), (seconds,))
        .await
        .expect("evm_increaseTime");
    let _: () = h
        .maker_provider
        .raw_request("anvil_mine".into(), (1u64,))
        .await
        .expect("anvil_mine");
}

// ---------------------------------------------------------------- wiring

async fn open_pool(dir: &Path, name: &str) -> SqlitePool {
    let url = format!("sqlite://{}?mode=rwc", dir.join(name).display());
    SqlitePool::connect(&url).await.expect("open sqlite")
}

fn assets(stack: &Stack) -> Arc<AssetManager> {
    let token = |address, symbol: &str| TokenMeta {
        chain_id: stack.chain_id,
        address,
        symbol: symbol.to_string(),
        name: symbol.to_string(),
        decimals: 18,
        logo_uri: None,
        tags: vec![],
    };
    Arc::new(AssetManager::new(
        TokenList {
            name: "e2e".to_string(),
            tokens: vec![token(stack.h.t0, "T0"), token(stack.h.t1, "T1")],
        },
        Arc::new(solvent_core::registry::SharedSnapshot::default()),
    ))
}

fn execution(
    h: &Harness,
    led: Arc<LedgerService>,
    pool: SqlitePool,
    redb: &Path,
) -> Arc<ExecutionService> {
    let key_hex = format!("0x{}", alloy::hex::encode(h.maker_signer.to_bytes()));
    let signer = LocalSigner::from_private_key(&key_hex).expect("local signer");
    let policy = DefaultPolicyEngine::new(
        vec![Box::new(AllowAll)],
        Arc::new(walletkit::adapters::SystemClock),
    );
    let transport = Transport::url(h.endpoint.parse().expect("endpoint url")).expect("transport");
    let store = RedbStateStore::open(redb).expect("redb");
    let wallet = Wallet::builder(Arc::new(transport), Arc::new(signer), Arc::new(policy))
        .store(Arc::new(store))
        .confirmations(1)
        .bump_timeout(0)
        .build();
    let exec = Arc::new(WalletkitExecutor::new(
        wallet,
        SubmissionOpts::public(),
        Arc::new(SqliteFillStore::new(pool)),
    ));
    let settlement = Arc::new(AquaSettlementReader::new(
        Arc::new(h.maker_provider.clone()),
        *h.aqua.address(),
    ));
    Arc::new(ExecutionService::new(exec.clone(), exec, settlement, led))
}

/// Mine and run the production reconcile worker until nothing is in flight. Settling the *trade*
/// is the worker's job, not the execution service's — the latter only closes out the ledger.
async fn drive(reconcile: &ReconcileService, svc: &ExecutionService, h: &Harness) {
    for _ in 0..10 {
        if svc.pending().await.expect("pending") == 0 {
            break;
        }
        let _: () = h
            .maker_provider
            .raw_request("anvil_mine".into(), (2u64,))
            .await
            .expect("anvil_mine");
        reconcile.tick().await.expect("reconcile tick");
    }
}

// ---------------------------------------------------------------- the test

#[tokio::test]
async fn e2e_the_loop_waits_for_the_price_then_fills_from_the_feed() {
    if common::skip_without_anvil() {
        return;
    }
    let stack = setup().await;
    let spec = first_supported(&stack.h);
    stack.h.ship(&spec).await;
    let (snapshot, _rp, _rd) = synced(&stack.h).await;
    let (svc_ledger, _ld) = ledger(&stack.h, &snapshot).await;
    let led = Arc::new(svc_ledger);
    // The loop routes against the ledger's published caps, so they must be populated the way the
    // composition root's budget poller populates them.
    led.sync_budgets(&snapshot.load())
        .await
        .expect("sync budgets");

    // The order: too expensive to source at its start amount, comfortably sourceable at its end.
    // The loop must therefore decline it, hold it, and act only once the decay has run.
    let unaffordable = spec.ship_hi;
    let affordable = spec.ship_hi / U256::from(10u64);
    let input = spec.ship_lo / U256::from(2u64);

    // The swapper must actually hold the input and have approved Permit2 — without it the fill
    // reverts inside the reactor's pull, which the sim gate correctly refuses to submit.
    MockERC20::new(stack.h.t0, stack.h.taker_provider.clone())
        .mint(stack.h.taker, input)
        .send()
        .await
        .expect("mint")
        .watch()
        .await
        .expect("mint mined");
    MockERC20::new(stack.h.t0, stack.h.taker_provider.clone())
        .approve(PERMIT2, U256::MAX)
        .send()
        .await
        .expect("approve permit2")
        .watch()
        .await
        .expect("approve mined");

    let created = chain_now(&stack.h).await;
    let decay_start = created + 24;
    let decay_end = decay_start + 60;
    let cosigner_key = PrivateKeySigner::random();
    let builder = SignedOrderBuilder::new(
        PERMIT2,
        stack.chain_id,
        stack.h.taker_signer.clone(),
        cosigner_key.clone(),
    );
    let order = OrderSpec {
        reactor: stack.reactor,
        nonce: U256::from(1u64),
        deadline: created + 600,
        input_token: stack.h.t0,
        input_start: input,
        input_end: input,
        output_token: stack.h.t1,
        output_start: unaffordable,
        output_end: affordable,
        recipient: RECIPIENT,
        decay_start,
        decay_end,
        exclusive_filler: ANOTHER_FILLER,
        exclusivity_override_bps: 100,
    };
    let raw = builder.build(&order, created);

    // Decode it once here purely to publish a truthful order hash, as the API does.
    let order_hash = UniswapXV2Normalizer::new(stack.reactor, vec![cosigner_key.address()])
        .normalize(&raw)
        .expect("the harness order normalizes")
        .id
        .0;

    // Publish it exactly as the Orders API would.
    let base = serve_orders(json!({"orders": [{
        "orderHash": format!("{order_hash:#x}"),
        "encodedOrder": format!("0x{}", alloy::hex::encode(&raw.payload)),
        "signature": format!("0x{}", alloy::hex::encode(&raw.signature)),
        "createdAt": created,
        "orderStatus": "open",
        "type": "Dutch_V2",
        "chainId": stack.chain_id,
    }]}))
    .await;

    // ---- the production stack, wired as the composition root does it ----
    let clock_now = Arc::new(parking_lot::Mutex::new(created));
    let clock = Arc::new(ChainClock(Arc::clone(&clock_now)));

    let normalizer: Arc<dyn Normalizer> = Arc::new(UniswapXV2Normalizer::new(
        stack.reactor,
        vec![cosigner_key.address()],
    ));
    let pipeline = Arc::new(IngestPipeline::new(
        BTreeMap::from([(ProtocolId::UniswapXV2, normalizer)]),
        Duration::from_secs(600),
        1024,
        Arc::clone(&clock) as Arc<dyn Clock>,
        Admission {
            supported_chains: BTreeSet::from([ChainId(stack.chain_id)]),
            tokens: BTreeSet::from([stack.h.t0, stack.h.t1]),
            max_outputs: 4,
        },
        None,
    ));
    let feed: Arc<dyn OrderFeed> = Arc::new(HostedFeed::new(
        Arc::new(
            OrdersApiClient::new(
                base,
                ChainId(stack.chain_id),
                "Dutch_V2".to_string(),
                ProtocolId::UniswapXV2,
            )
            .expect("client"),
        ),
        ChainId(stack.chain_id),
        vec![Scope::Book],
        Duration::from_millis(20),
        Arc::new(FeedHealth::default()),
    ));

    let dir = tempfile::tempdir().expect("tempdir");
    let trade_pool = open_pool(dir.path(), "trades.db").await;
    let trades = Arc::new(SqliteTradeStore::new(trade_pool.clone()));
    trades.migrate().await.expect("migrate trades");
    let trades: Arc<dyn TradeStore> = trades;

    let exec_pool = open_pool(dir.path(), "exec.db").await;
    SqliteFillStore::new(exec_pool.clone())
        .migrate()
        .await
        .expect("migrate exec");
    let execution = execution(
        &stack.h,
        Arc::clone(&led),
        exec_pool,
        &dir.path().join("wallet.redb"),
    );

    let market = MarketCache::new();
    let gas: Arc<dyn GasPrice> = market.clone();
    let oracle: Arc<dyn PriceOracle> = market;
    let valuation = Arc::new(Valuation::new(Arc::clone(&oracle)));
    let leg_cost = Arc::new(LegCostResolver::new(
        gas,
        oracle,
        assets(&stack),
        Address::ZERO,
        0,
    ));
    let fill_builder: Arc<dyn FillBuilder> = Arc::new(stack.fill_builder());
    let routing = RoutingConfig::new(16, 4, 0);
    let swap = Arc::new(SwapService::new(
        Arc::clone(&snapshot),
        Arc::clone(&led),
        Arc::new(StrategyGuard::default()),
        Arc::clone(&trades),
        Arc::clone(&execution),
        fill_builder,
        Arc::clone(&leg_cost),
        Arc::clone(&clock) as Arc<dyn Clock>,
        SwapConfig {
            routing,
            chain_id: stack.chain_id,
            filler: stack.filler,
            filler_owner: stack.h.maker,
            reservation_ttl_secs: 600,
        },
    ));
    let decision = Arc::new(DecisionService::new(
        DecisionDeps {
            registry: Arc::clone(&snapshot),
            ledger: Arc::clone(&led),
            swap: Arc::clone(&swap),
            leg_cost: Arc::clone(&leg_cost),
            valuation: Arc::clone(&valuation),
            clock: Arc::clone(&clock) as Arc<dyn Clock>,
            order_log: None,
        },
        DecisionConfig {
            routing,
            filler: stack.filler,
            max_tracked: 64,
        },
    ));

    // ---- run it ----
    let (tx, rx) = mpsc::channel::<Intent>(16);
    let feeder = tokio::spawn({
        let pipeline = Arc::clone(&pipeline);
        async move { pipeline.run(vec![feed], tx).await }
    });

    // A tick stream the test drives by hand, so "the loop waited" is an assertion and not a sleep.
    let (tick_tx, tick_rx) = mpsc::channel::<()>(8);
    let loop_handle = tokio::spawn({
        let decision = Arc::clone(&decision);
        async move { decision.run(rx, Box::pin(tokio_stream_from(tick_rx))).await }
    });

    // Tick while the order is still inside the exclusivity window and priced at its start amount:
    // it must not fill.
    for _ in 0..3 {
        tick_tx.send(()).await.expect("tick");
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
    // No trade row at all: the order is being *held*, not attempted and declined. A declined
    // attempt would still persist a trade, and would have consumed the loop's single shot at it.
    let early = trades.stats().await.expect("stats");
    assert_eq!(
        (early.settled, early.confirmed, early.failed),
        (0, 0, 0),
        "the loop must not have committed while the order was unaffordable"
    );
    assert_eq!(
        execution.pending().await.expect("pending"),
        0,
        "and nothing was submitted"
    );

    // Move past the decay: the required output falls to what the makers can cover.
    advance(&stack.h, 200).await;
    *clock_now.lock() = chain_now(&stack.h).await;

    let before = balance_of(&stack.h, stack.h.t1, RECIPIENT).await;
    for _ in 0..5 {
        tick_tx.send(()).await.expect("tick");
        tokio::time::sleep(Duration::from_millis(120)).await;
        if execution.pending().await.expect("pending") > 0 {
            break;
        }
    }
    assert!(
        execution.pending().await.expect("pending") > 0,
        "the loop should have submitted once the price crossed"
    );

    let reconcile = ReconcileService::new(
        Arc::clone(&execution),
        Arc::clone(&trades),
        Arc::clone(&led),
        Arc::clone(&clock) as Arc<dyn Clock>,
    );
    drive(&reconcile, &execution, &stack.h).await;
    assert_eq!(execution.pending().await.expect("pending"), 0, "it settled");

    // The swapper's recipient was paid on chain, by a fill nobody in this test chose to make.
    let delivered = balance_of(&stack.h, stack.h.t1, RECIPIENT).await - before;
    assert_eq!(
        delivered, affordable,
        "the settled output is the decayed amount"
    );

    let stats = trades.stats().await.expect("stats");
    assert_eq!(stats.confirmed, 1, "one confirmed trade");

    drop(tick_tx);
    let _ = loop_handle.await;
    feeder.abort();
}

/// A `Stream` over a channel, so the test can step the loop deterministically.
fn tokio_stream_from(mut rx: mpsc::Receiver<()>) -> impl futures::Stream<Item = ()> {
    futures::stream::poll_fn(move |cx| rx.poll_recv(cx))
}
