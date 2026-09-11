//! The production 1inch fill path, hermetically, end to end: a signed order goes through the real
//! `IngestPipeline`/`Admission`/`OneInchNormalizer`, the real `DecisionService`/`SwapService`, and
//! settles via `OneInchFillBuilder` against a source-deployed real 1inch `LimitOrderProtocol` +
//! Aqua/SwapVM — the same production types the composition root wires, minus only the feed's own
//! HTTP polling (covered separately by the adapter-level `orders_api`/feed tests) and the mainnet
//! address 1inch's real router happens to live at (irrelevant here: `OneInchFillBuilder` targets
//! whichever `OneInchLimitOrderAquaFiller` it is constructed with, not `Intent::settler`). Gated on
//! anvil.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::signers::{local::PrivateKeySigner, Signer};
use alloy::sol;
use alloy::sol_types::SolValue;
use futures::stream::BoxStream;

use common::{first_supported, ledger, synced, Harness, MockERC20};
use solvent_adapters::execution::{AquaSettlementReader, SqliteFillStore, WalletkitExecutor};
use solvent_adapters::ingest::oneinch::{OneInchFillBuilder, OneInchNormalizer};
use solvent_adapters::ledger::SystemClock;
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
use solvent_core::primitives::ingest::{Intent, OrderSource, ProtocolId, RawOrder};
use solvent_core::primitives::routing::RoutingConfig;
use solvent_core::primitives::ChainId;
use solvent_core::reconcile::ReconcileService;
use solvent_core::routing::{LegCostResolver, StrategyGuard};
use solvent_core::swap::{SwapConfig, SwapService};
use solvent_core::valuation::Valuation;
use walletkit::adapters::policy::{AllowAll, DefaultPolicyEngine};
use walletkit::adapters::{
    LocalSigner, RedbStateStore, SystemClock as WalletSystemClock, Transport,
};
use walletkit::core::deps::SubmissionOpts;
use walletkit::Wallet;

// `router_address`/`fee_taker_address` in the production normalizer only recognize mainnet
// (chain id 1) — by design, since 1inch's real router has one fixed address. This tag is purely
// Solvent's own off-chain bookkeeping field; it need not match anvil's actual `--chain-id`, and
// the contracts deployed below sit at whatever address anvil gives them, not the real mainnet one
// (`OneInchFillBuilder` never reads `Intent::settler`, only its own configured filler address).
const ONE_INCH_CHAIN: ChainId = ChainId(1);

mod weth_abi {
    use super::sol;
    sol!(
        #[sol(rpc)]
        WETHMock,
        "tests/fixtures/artifacts/WETHMock.json"
    );
}
use weth_abi::WETHMock;

// `LimitOrderProtocol`/`OneInchLimitOrderAquaFiller`'s ABIs reference 1inch's own `Address`/
// `MakerTraits` custom value types by name (via `internalType`); unlike a plain inline Solidity
// block, `sol!`'s JSON-artifact form expects those types already in scope rather than generating
// them, and declaring them here would collide with `alloy::primitives::Address`. Simplest to just
// deploy their raw bytecode directly (`deploy_bytecode` below) and call them through our own plain
// interface, exactly as `oneinch/fill.rs`'s production code already does for calldata encoding.
sol! {
    struct Order {
        uint256 salt;
        address maker;
        address receiver;
        address makerAsset;
        address takerAsset;
        uint256 makingAmount;
        uint256 takingAmount;
        uint256 makerTraits;
    }
    struct WireOrder {
        Order order;
        bytes extension;
    }

    // 1inch's real `IOrderMixin.Order` types `maker`/`receiver`/`makerAsset`/`takerAsset`/
    // `makerTraits` as its own `Address`/`MakerTraits` custom value types (uint256 wrappers) — the
    // ABI *encoding* of those and plain `address`/`uint256` is identical, but the function
    // *selector* is computed from the canonical (uint256) name, not the plain-`address` one. A
    // struct declared with plain `address` fields therefore calls the wrong selector. This mirrors
    // the real interface exactly for that purpose; `RealOrder::from` below does the conversion.
    struct RealOrder {
        uint256 salt;
        uint256 maker;
        uint256 receiver;
        uint256 makerAsset;
        uint256 takerAsset;
        uint256 makingAmount;
        uint256 takingAmount;
        uint256 makerTraits;
    }

    #[sol(rpc)]
    interface ILimitOrderProtocolView {
        function hashOrder(RealOrder calldata order) external view returns (bytes32);
    }
}

impl From<Order> for RealOrder {
    fn from(o: Order) -> RealOrder {
        let addr_to_u256 = |a: Address| U256::from_be_slice(a.as_slice());
        RealOrder {
            salt: o.salt,
            maker: addr_to_u256(o.maker),
            receiver: addr_to_u256(o.receiver),
            makerAsset: addr_to_u256(o.makerAsset),
            takerAsset: addr_to_u256(o.takerAsset),
            makingAmount: o.makingAmount,
            takingAmount: o.takingAmount,
            makerTraits: o.makerTraits,
        }
    }
}

/// Deploy the contract compiled at `artifact_json` (a forge artifact: `include_str!`'d, with a
/// `bytecode.object` field), appending ABI-encoded constructor args. Used for the two contracts
/// above instead of `sol!`'s own `::deploy` convenience, which needs their custom value types
/// resolvable (see the note above).
async fn deploy_bytecode(
    provider: &alloy::providers::DynProvider,
    artifact_json: &str,
    constructor_args: &[u8],
) -> Address {
    use alloy::{
        primitives::Bytes as AlloyBytes, providers::Provider, rpc::types::TransactionRequest,
    };
    let artifact: serde_json::Value = serde_json::from_str(artifact_json).expect("artifact json");
    let bytecode_hex = artifact["bytecode"]["object"]
        .as_str()
        .expect("bytecode.object");
    let mut init_code: Vec<u8> = alloy::hex::decode(bytecode_hex).expect("decode bytecode");
    init_code.extend_from_slice(constructor_args);
    let receipt = provider
        .send_transaction(
            TransactionRequest::default()
                .create()
                .input(AlloyBytes::from(init_code).into()),
        )
        .await
        .expect("send deploy tx")
        .get_receipt()
        .await
        .expect("deploy receipt");
    assert!(receipt.status(), "constructor reverted: {receipt:?}");
    let addr = receipt.contract_address.expect("contract creation address");
    let code = provider.get_code_at(addr).await.expect("get_code_at");
    assert!(!code.is_empty(), "no code landed at {addr}");
    addr
}

/// Yields exactly the one pre-built `RawOrder` this test hand-signs, then ends — a stand-in for
/// `OneInchFeed`'s HTTP polling, which is tested on its own elsewhere. Everything downstream of
/// this (`IngestPipeline`, `Admission`, `OneInchNormalizer`, `DecisionService`, `SwapService`,
/// `OneInchFillBuilder`) is the real production type.
struct OneShotFeed(std::sync::Mutex<Option<RawOrder>>);

impl OrderFeed for OneShotFeed {
    fn stream(&self) -> BoxStream<'static, RawOrder> {
        // Never ends — a real feed's stream doesn't either, and `IngestPipeline::run` returning
        // early (dropping its output sender) would close `DecisionService`'s intent channel and
        // end its loop before a single tick lands.
        use futures::StreamExt;
        let order = self.0.lock().expect("lock").take();
        Box::pin(futures::stream::iter(order).chain(futures::stream::pending()))
    }
}

fn assets(h: &Harness, chain_id: u64) -> Arc<AssetManager> {
    let token = |address, symbol: &str| TokenMeta {
        chain_id,
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
            tokens: vec![token(h.t0, "T0"), token(h.t1, "T1")],
        },
        Arc::new(solvent_core::registry::SharedSnapshot::default()),
    ))
}

fn execution(
    h: &Harness,
    led: Arc<LedgerService>,
    pool: sqlx::SqlitePool,
    redb: &std::path::Path,
) -> Arc<ExecutionService> {
    let key_hex = format!("0x{}", alloy::hex::encode(h.maker_signer.to_bytes()));
    let signer = LocalSigner::from_private_key(&key_hex).expect("local signer");
    let policy = DefaultPolicyEngine::new(vec![Box::new(AllowAll)], Arc::new(WalletSystemClock));
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

/// Mine and run the production reconcile worker until nothing is in flight.
async fn drive(reconcile: &ReconcileService, svc: &ExecutionService, h: &Harness) {
    use alloy::providers::Provider;
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

#[tokio::test]
async fn e2e_oneinch_intent_fills_through_the_production_stack() {
    if common::skip_without_anvil() {
        return;
    }
    let h = Harness::setup().await;
    common::etch_multicall3(&h).await;
    let chain_id = h.maker_provider.get_chain_id().await.expect("chain id");
    let spec = first_supported(&h);
    h.ship(&spec).await;
    let (snapshot, _rp, _rd) = synced(&h).await;
    let (svc_ledger, _ld) = ledger(&h, &snapshot).await;
    let led = Arc::new(svc_ledger);
    led.sync_budgets(&snapshot.load())
        .await
        .expect("sync budgets");

    // Deploy a real 1inch Limit Order Protocol v4 (needs a WETH-like token, unused otherwise) and
    // our filler, targeting it — independent contracts from the UniswapX reactor/filler `setup()`
    // also deploys, sharing only Aqua/SwapVM and the two tokens.
    let weth = WETHMock::deploy(h.maker_provider.clone())
        .await
        .expect("deploy WETH mock");
    let protocol_address = deploy_bytecode(
        &h.maker_provider,
        include_str!("fixtures/artifacts/LimitOrderProtocol.json"),
        &weth.address().abi_encode(),
    )
    .await;
    let filler_address = deploy_bytecode(
        &h.maker_provider,
        include_str!("fixtures/artifacts/OneInchLimitOrderAquaFiller.json"),
        &(h.maker, protocol_address).abi_encode_params(),
    )
    .await;
    let protocol = ILimitOrderProtocolView::new(protocol_address, h.maker_provider.clone());

    // The order's signer: gives t0 (makerAsset), wants t1 (takerAsset) — same roles UniswapX's
    // swapper plays in the sibling E2Es, output sized to what the shipped strategy can source.
    let signer_key = PrivateKeySigner::random();
    let signer_address = signer_key.address();
    let making_amount = spec.ship_lo / U256::from(2u64);
    let taking_amount = spec.ship_hi / U256::from(10u64);

    MockERC20::new(h.t0, h.maker_provider.clone())
        .mint(signer_address, making_amount)
        .send()
        .await
        .expect("mint")
        .watch()
        .await
        .expect("mint mined");
    let signer_provider = {
        use alloy::{network::EthereumWallet, providers::Provider, providers::ProviderBuilder};
        ProviderBuilder::new()
            .wallet(EthereumWallet::from(signer_key.clone()))
            .connect_http(h.endpoint.parse().expect("endpoint"))
            .erased()
    };
    // A freshly-random signer starts with no ETH; fund it for the approve tx's gas.
    {
        use alloy::providers::ext::AnvilApi;
        h.maker_provider
            .anvil_set_balance(signer_address, U256::from(10u64).pow(U256::from(18u64)))
            .await
            .expect("fund signer with gas");
    }
    MockERC20::new(h.t0, signer_provider.clone())
        .approve(*protocol.address(), U256::MAX)
        .send()
        .await
        .expect("approve protocol")
        .watch()
        .await
        .expect("approve mined");

    // A real, finite expiration (bits 80..120 of makerTraits) — `0` means "never expires", which
    // decodes as `deadline = u64::MAX`, too large for the trade store's i64 column. No real order
    // is shaped that way either.
    let expiration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("now")
        .as_secs()
        + 3600;
    let order = Order {
        salt: U256::from(1u64),
        maker: signer_address,
        receiver: Address::ZERO,
        makerAsset: h.t0,
        takerAsset: h.t1,
        makingAmount: making_amount,
        takingAmount: taking_amount,
        makerTraits: U256::from(expiration) << 80,
    };
    // The real deployed contract's own EIP-712 digest — signing this (not a locally-recomputed
    // domain) is what makes the on-chain signature check pass regardless of what chain id or
    // domain name/version this specific deployment actually carries.
    let order_hash = protocol
        .hashOrder(RealOrder::from(order.clone()))
        .call()
        .await
        .expect("hashOrder");
    let sig = signer_key
        .sign_hash(&order_hash)
        .await
        .expect("sign order hash");
    let signature = Bytes::from(sig.as_bytes().to_vec());

    let payload = Bytes::from(
        WireOrder {
            order,
            extension: Bytes::new(),
        }
        .abi_encode(),
    );
    let raw = RawOrder::new(
        ProtocolId::OneInchLimitOrder,
        ONE_INCH_CHAIN,
        payload,
        signature,
        0,
        OrderSource::OneInch,
    );

    // ---- the production stack, wired as the composition root does it ----
    let clock = Arc::new(SystemClock);

    let normalizer: Arc<dyn Normalizer> = Arc::new(OneInchNormalizer);
    let pipeline = Arc::new(IngestPipeline::new(
        BTreeMap::from([(ProtocolId::OneInchLimitOrder, normalizer)]),
        Duration::from_secs(600),
        1024,
        Arc::clone(&clock) as Arc<dyn Clock>,
        Admission {
            supported_chains: BTreeSet::from([ONE_INCH_CHAIN]),
            tokens: BTreeSet::from([h.t0, h.t1]),
            max_outputs: 4,
        },
        None,
    ));
    let feed: Arc<dyn OrderFeed> = Arc::new(OneShotFeed(std::sync::Mutex::new(Some(raw))));

    let dir = tempfile::tempdir().expect("tempdir");
    let trade_pool = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}?mode=rwc",
        dir.path().join("trades.db").display()
    ))
    .await
    .expect("open trades sqlite");
    let trades = Arc::new(SqliteTradeStore::new(trade_pool));
    trades.migrate().await.expect("migrate trades");
    let trades: Arc<dyn TradeStore> = trades;

    let exec_pool = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}?mode=rwc",
        dir.path().join("exec.db").display()
    ))
    .await
    .expect("open exec sqlite");
    SqliteFillStore::new(exec_pool.clone())
        .migrate()
        .await
        .expect("migrate exec");
    let execution = execution(
        &h,
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
        assets(&h, chain_id),
        Address::ZERO,
        0,
    ));
    let fill_builders: BTreeMap<ProtocolId, Arc<dyn FillBuilder>> = BTreeMap::from([(
        ProtocolId::OneInchLimitOrder,
        Arc::new(OneInchFillBuilder::new(h.app, filler_address)) as Arc<dyn FillBuilder>,
    )]);
    let routing = RoutingConfig::new(16, 4, 0);
    let swap = Arc::new(SwapService::new(
        Arc::clone(&snapshot),
        Arc::clone(&led),
        Arc::new(StrategyGuard::default()),
        Arc::clone(&trades),
        Arc::clone(&execution),
        fill_builders,
        Arc::clone(&leg_cost),
        Arc::clone(&clock) as Arc<dyn Clock>,
        SwapConfig {
            routing,
            chain_id,
            filler: Address::ZERO, // unused: 1inch intents never carry UniswapX-style exclusivity
            filler_owner: h.maker,
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
            filler: Address::ZERO, // unused: 1inch intents never carry UniswapX-style exclusivity
            max_tracked: 64,
        },
    ));

    // ---- run it ----
    let (tx, rx) = tokio::sync::mpsc::channel::<Intent>(16);
    let feeder = tokio::spawn({
        let pipeline = Arc::clone(&pipeline);
        async move { pipeline.run(vec![feed], tx).await }
    });

    let (tick_tx, tick_rx) = tokio::sync::mpsc::channel::<()>(8);
    let loop_handle = tokio::spawn({
        let decision = Arc::clone(&decision);
        async move { decision.run(rx, Box::pin(tokio_stream_from(tick_rx))).await }
    });

    for _ in 0..10 {
        tick_tx.send(()).await.expect("tick");
        tokio::time::sleep(Duration::from_millis(120)).await;
        if execution.pending().await.expect("pending") > 0 {
            break;
        }
    }
    assert!(
        execution.pending().await.expect("pending") > 0,
        "the loop should have filled the 1inch intent"
    );

    let reconcile = ReconcileService::new(
        Arc::clone(&execution),
        Arc::clone(&trades),
        Arc::clone(&led),
        Arc::clone(&clock) as Arc<dyn Clock>,
    );
    drive(&reconcile, &execution, &h).await;
    assert_eq!(execution.pending().await.expect("pending"), 0, "it settled");

    let signer_balance = MockERC20::new(h.t1, h.maker_provider.clone())
        .balanceOf(signer_address)
        .call()
        .await
        .expect("balanceOf");
    assert_eq!(
        signer_balance, taking_amount,
        "the order's signer received the real takerAsset payout"
    );

    let stats = trades.stats().await.expect("stats");
    assert_eq!(stats.confirmed, 1, "one confirmed trade");

    drop(tick_tx);
    let _ = loop_handle.await;
    feeder.abort();
}

fn tokio_stream_from(mut rx: tokio::sync::mpsc::Receiver<()>) -> impl futures::Stream<Item = ()> {
    futures::stream::poll_fn(move |cx| rx.poll_recv(cx))
}
