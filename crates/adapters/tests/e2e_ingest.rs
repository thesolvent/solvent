//! Live full-loop E2E: a self-hosted UniswapX order → normalize → route → reserve → **fill on-chain**
//! through the deployed reactor + `UniswapXAquaFiller` via a direct `fill()` call, sourcing the
//! output from a shipped Aqua maker. Proves the ingest→fill path against the real contracts; the
//! sibling `e2e_execution` drives the same loop through the production execution service. Gated on anvil.

mod common;

use std::{collections::HashMap, sync::Arc, time::Duration};

use alloy::primitives::{address, keccak256, Address, Bytes, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::{TransactionInput, TransactionRequest};
use alloy::signers::{local::PrivateKeySigner, SignerSync};
use alloy::sol_types::SolValue;
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use futures::StreamExt;
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;
use ulid::Ulid;

use common::{
    balance_of, budget_source, drive_execution, execution_service, ledger, pipeline, quote_caps,
    rid, setup, MockERC20, Stack, StrategySpec, CHAIN, PERMIT2,
};
use solvent_adapters::balances::AlloyBalancesOracle;
use solvent_adapters::chain::ChainHead;
use solvent_adapters::execution::LocalPolicySigner;
use solvent_adapters::http::state::{AppConfig, AppState, Features};
use solvent_adapters::http::{router, DepthReader};
use solvent_adapters::ingest::erc7683::{Erc7683Normalizer, Solvent7683Order};
use solvent_adapters::ingest::uniswapx::{
    OrderSpec, SelfHostedFeed, ServerCosigner, SignedOrderBuilder, UniswapXV2Normalizer,
};
use solvent_adapters::ingest::ProtocolFillBuilder;
use solvent_adapters::ledger::SystemClock;
use solvent_adapters::metrics::{SqliteMakerMetrics, SqliteQuoteLog};
use solvent_adapters::rebate::{FillerRebateCallBuilder, SqliteRebateStore};
use solvent_adapters::registry::{AlloyBlockTimes, SqliteStore};
use solvent_adapters::routing::{BinanceHistory, MarketCache};
use solvent_adapters::trade::SqliteTradeStore;
use solvent_core::asset::{AssetManager, PairHistoryService, TokenList, TokenMeta};
use solvent_core::balances::BalancesService;
use solvent_core::deps::ingest::{FillBuilder, Normalizer, OrderFeed};
use solvent_core::deps::rebate::{RebateCallBuilder, RebateMarketBook};
use solvent_core::deps::routing::{GasPrice, PriceOracle};
use solvent_core::deps::trade::TradeStore;
use solvent_core::execution::ExecutionService;
use solvent_core::maker::MakerService;
use solvent_core::pool::{DepthService, PoolService};
use solvent_core::primitives::ingest::{ExecutionFeePolicy, OrderSource, ProtocolId, RawOrder};
use solvent_core::primitives::ledger::{AccountKey, ReservationSource};
use solvent_core::primitives::rebate::{RebateAllocation, RebatePlan};
use solvent_core::primitives::routing::{RouteRequest, RoutingConfig};
use solvent_core::primitives::trade::TradeId;
use solvent_core::primitives::{ChainId, RebateBatchId, ReservationId};
use solvent_core::quote::QuoteService;
use solvent_core::rebate::{
    RebateMarketData, RebatePolicy, RebatePolicyConfig, RebateService, RebateServiceConfig,
};
use solvent_core::registry::price;
use solvent_core::registry::SharedSnapshot;
use solvent_core::routing::{route, GuardSnapshot, LegCostResolver, RoutingBook, StrategyGuard};
use solvent_core::swap::{SwapConfig, SwapService};
use solvent_core::trade::TradeService;
use solvent_core::valuation::Valuation;

#[tokio::test]
async fn e2e_protected_curves_support_user_fills_and_rebates() {
    if common::skip_without_anvil() {
        return;
    }
    for label in ["xyc", "concentrate", "pegged_18_18"] {
        exercise_protected_curve(label).await;
    }
}

#[tokio::test]
async fn e2e_erc7683_order_routes_and_settles_through_aqua() {
    if common::skip_without_anvil() {
        return;
    }
    let stack = setup().await;
    let h = &stack.h;
    let spec = strategy(h, "xyc");
    h.ship(&spec).await;
    let (registry_sync, snapshot, _rp, _rd) = pipeline(h, CHAIN).await;
    registry_sync
        .sync_once(h.latest_block().await)
        .await
        .expect("initial registry sync");
    let (ledger, _ld) = ledger(h, &snapshot).await;
    let snap = snapshot.load();
    let budgets = budget_source(h, &snapshot);
    let caps = quote_caps(&snap, &budgets, h.t1, h.t0).await;
    let output = spec.ship_hi / U256::from(10u64);
    let routing = RoutingConfig::new(64, 8, 150_000);
    let provisional = RouteRequest {
        intent: solvent_core::primitives::IntentId(B256::ZERO),
        token_in: h.t0,
        token_out: h.t1,
        amount: output,
        exact_in: false,
    };
    let provisional_plan = route(
        RoutingBook::new(&snap, &caps, &GuardSnapshot::default()),
        &provisional,
        spec.ship_lo / U256::from(2u64),
        &routing,
        U256::ZERO,
        None,
    )
    .plan
    .expect("provisional route");
    let routed_input = provisional_plan
        .legs
        .iter()
        .fold(U256::ZERO, |sum, leg| sum + leg.amount_in);
    let fee_policy = ExecutionFeePolicy::new(5).expect("fee policy");
    let mut gross_input = routed_input + U256::from(1u8);
    loop {
        let next = routed_input + fee_policy.fee(gross_input).expect("executor fee");
        if next == gross_input {
            break;
        }
        gross_input = next;
    }
    let executor_fee = gross_input - routed_input;
    let now = h
        .maker_provider
        .get_block(alloy::eips::BlockId::latest())
        .await
        .expect("block")
        .expect("latest block")
        .header
        .timestamp;
    let order = Solvent7683Order {
        settler: stack.erc7683_settler,
        user: h.taker,
        chainId: U256::from(stack.chain_id),
        inputToken: h.t0,
        inputAmount: gross_input,
        outputToken: h.t1,
        outputAmount: output,
        recipient: h.taker,
        executorFee: executor_fee,
        nonce: U256::from(77u8),
        deadline: U256::from(now + 1_000),
    };
    let signature = h
        .taker_signer
        .sign_hash_sync(&order.permit_digest(PERMIT2, stack.chain_id))
        .expect("sign Permit2 witness");
    let raw = RawOrder::new(
        ProtocolId::Erc7683,
        ChainId(stack.chain_id),
        Bytes::from(order.abi_encode()),
        Bytes::from(signature.as_bytes()),
        now,
        OrderSource::Solvent,
    );
    let normalized = Erc7683Normalizer::new(
        ChainId(stack.chain_id),
        PERMIT2,
        stack.erc7683_settler,
        fee_policy,
    )
    .normalize_order(&raw)
    .expect("normalize ERC-7683 order");
    let request = RouteRequest {
        intent: normalized.intent.id,
        token_in: h.t0,
        token_out: h.t1,
        amount: output,
        exact_in: false,
    };
    let plan = route(
        RoutingBook::new(&snap, &caps, &GuardSnapshot::default()),
        &request,
        normalized
            .intent
            .routing_input_limit
            .expect("routing budget"),
        &routing,
        U256::ZERO,
        None,
    )
    .plan
    .expect("final route");
    let sources = plan
        .legs
        .iter()
        .map(|leg| ReservationSource {
            maker: leg.maker,
            strategy_hash: leg.strategy_hash,
            token: leg.token_out,
            amount: leg.amount_out,
        })
        .collect();
    ledger
        .reserve(rid(2), normalized.intent.id, sources, 60)
        .await
        .expect("reserve ERC-7683 route");
    MockERC20::new(h.t0, h.taker_provider.clone())
        .mint(h.taker, gross_input)
        .send()
        .await
        .expect("mint input")
        .watch()
        .await
        .expect("mint input mined");
    MockERC20::new(h.t0, h.taker_provider.clone())
        .approve(PERMIT2, U256::MAX)
        .send()
        .await
        .expect("approve Permit2")
        .watch()
        .await
        .expect("Permit2 approval mined");
    let executor_before = balance_of(h, h.t0, h.maker).await;
    let fill = stack
        .erc7683_fill_builder()
        .build(&normalized.intent, &plan, &snap)
        .await
        .expect("ERC-7683 fill");
    assert_eq!(fill.target, stack.erc7683_filler);
    let receipt = h
        .maker_provider
        .send_transaction(
            TransactionRequest::default()
                .to(fill.target)
                .input(TransactionInput::new(fill.calldata)),
        )
        .await
        .expect("send ERC-7683 fill")
        .get_receipt()
        .await
        .expect("ERC-7683 receipt");

    assert!(receipt.status(), "ERC-7683 fill reverted");
    assert_eq!(balance_of(h, h.t1, h.taker).await, output);
    assert_eq!(
        balance_of(h, h.t0, h.maker).await,
        executor_before + gross_input
    );
}

#[tokio::test]
async fn e2e_erc7683_post_swap_submits_through_execution_service() {
    if common::skip_without_anvil() {
        return;
    }
    let stack = setup().await;
    let h = &stack.h;
    let spec = strategy(h, "xyc");
    h.ship(&spec).await;
    let (registry_sync, snapshot, _rp, _rd) = pipeline(h, CHAIN).await;
    registry_sync
        .sync_once(h.latest_block().await)
        .await
        .expect("initial registry sync");
    let (ledger, _ledger_dir) = ledger(h, &snapshot).await;
    let ledger = Arc::new(ledger);
    ledger
        .sync_budgets(&snapshot.load())
        .await
        .expect("sync ledger budgets");

    let output = spec.ship_hi / U256::from(10u64);
    let routing = RoutingConfig::new(64, 8, 150_000);
    let provisional = RouteRequest {
        intent: solvent_core::primitives::IntentId(B256::ZERO),
        token_in: h.t0,
        token_out: h.t1,
        amount: output,
        exact_in: false,
    };
    let provisional_plan = route(
        RoutingBook::new(
            &snapshot.load(),
            &ledger.snapshot(),
            &GuardSnapshot::default(),
        ),
        &provisional,
        spec.ship_lo / U256::from(2u64),
        &routing,
        U256::ZERO,
        None,
    )
    .plan
    .expect("provisional route");
    let routed_input = provisional_plan
        .legs
        .iter()
        .fold(U256::ZERO, |sum, leg| sum + leg.amount_in);
    let fee_policy = ExecutionFeePolicy::new(5).expect("fee policy");
    let mut gross_input = routed_input + U256::from(1u8);
    loop {
        let next = routed_input + fee_policy.fee(gross_input).expect("executor fee");
        if next == gross_input {
            break;
        }
        gross_input = next;
    }
    let now = h
        .maker_provider
        .get_block(alloy::eips::BlockId::latest())
        .await
        .expect("block")
        .expect("latest block")
        .header
        .timestamp;
    let order = Solvent7683Order {
        settler: stack.erc7683_settler,
        user: h.taker,
        chainId: U256::from(stack.chain_id),
        inputToken: h.t0,
        inputAmount: gross_input,
        outputToken: h.t1,
        outputAmount: output,
        recipient: h.taker,
        executorFee: gross_input - routed_input,
        nonce: U256::from(78u8),
        deadline: U256::from(now + 1_000),
    };
    let signature = h
        .taker_signer
        .sign_hash_sync(&order.permit_digest(PERMIT2, stack.chain_id))
        .expect("sign Permit2 witness");
    MockERC20::new(h.t0, h.taker_provider.clone())
        .mint(h.taker, gross_input)
        .send()
        .await
        .expect("mint input")
        .watch()
        .await
        .expect("mint input mined");
    MockERC20::new(h.t0, h.taker_provider.clone())
        .approve(PERMIT2, U256::MAX)
        .send()
        .await
        .expect("approve Permit2")
        .watch()
        .await
        .expect("Permit2 approval mined");

    let execution_dir = tempfile::tempdir().expect("execution tempdir");
    let execution = Arc::new(execution_service(
        h,
        Arc::clone(&ledger),
        common::execution_pool(execution_dir.path()).await,
        &execution_dir.path().join("wallet.redb"),
    ));
    let (state, _state_dir) = erc7683_http_state(
        &stack,
        Arc::clone(&snapshot),
        Arc::clone(&ledger),
        Arc::clone(&execution),
        fee_policy,
    )
    .await;
    let body = serde_json::json!({
        "protocol": "erc7683",
        "encodedOrder": format!("0x{}", alloy::hex::encode(order.abi_encode())),
        "signature": format!("0x{}", alloy::hex::encode(signature.as_bytes())),
        "chainId": stack.chain_id,
    });
    let response = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/swap")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("HTTP response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let response: serde_json::Value = serde_json::from_slice(&body).expect("response JSON");
    assert_eq!(response["result"]["status"], "submitted");

    drive_execution(&execution, h).await;
    assert_eq!(execution.pending().await.expect("pending fills"), 0);
    assert_eq!(balance_of(h, h.t1, h.taker).await, output);
}

async fn erc7683_http_state(
    stack: &Stack,
    registry: Arc<SharedSnapshot>,
    ledger: Arc<solvent_core::ledger::LedgerService>,
    execution: Arc<ExecutionService>,
    fee_policy: ExecutionFeePolicy,
) -> (AppState, TempDir) {
    let state_dir = tempfile::tempdir().expect("state tempdir");
    let state_url = format!(
        "sqlite://{}?mode=rwc",
        state_dir.path().join("state.db").display()
    );
    let state_pool = SqlitePool::connect(&state_url)
        .await
        .expect("open state sqlite");
    let trades = Arc::new(SqliteTradeStore::new(state_pool.clone()));
    trades.migrate().await.expect("migrate trades");
    let trades: Arc<dyn TradeStore> = trades;

    let assets = Arc::new(AssetManager::new(
        TokenList {
            name: "Anvil ERC-7683".to_string(),
            tokens: vec![
                TokenMeta {
                    chain_id: stack.chain_id,
                    address: stack.h.t0,
                    symbol: "TKA".to_string(),
                    name: "Token A".to_string(),
                    decimals: 18,
                    logo_uri: None,
                    tags: Vec::new(),
                },
                TokenMeta {
                    chain_id: stack.chain_id,
                    address: stack.h.t1,
                    symbol: "TKB".to_string(),
                    name: "Token B".to_string(),
                    decimals: 18,
                    logo_uri: None,
                    tags: Vec::new(),
                },
            ],
        },
        Arc::clone(&registry),
    ));
    let market = MarketCache::new();
    let price_oracle: Arc<dyn PriceOracle> = market.clone();
    let gas: Arc<dyn GasPrice> = market.clone();
    let rebate_market: Arc<dyn RebateMarketBook> = market;
    let valuation = Arc::new(Valuation::new(Arc::clone(&price_oracle)));
    let clock = Arc::new(SystemClock);
    let leg_cost = Arc::new(LegCostResolver::new(
        gas.clone(),
        price_oracle,
        Arc::clone(&assets),
        Address::ZERO,
        150_000,
    ));
    let balances = Arc::new(BalancesService::new(
        Arc::new(AlloyBalancesOracle::new(
            stack.h.maker_provider.clone(),
            *stack.h.aqua.address(),
        )),
        Arc::clone(&assets),
        Arc::clone(&valuation),
    ));
    let metrics = Arc::new(SqliteMakerMetrics::new(state_pool.clone()));
    let events: Arc<dyn solvent_core::deps::registry::EventStore> =
        Arc::new(SqliteStore::new(state_pool.clone()));
    let block_times = Arc::new(AlloyBlockTimes::new(Arc::new(
        stack.h.maker_provider.clone(),
    )));
    let makers = Arc::new(MakerService::new(
        Arc::clone(&registry),
        Arc::clone(&assets),
        Arc::clone(&valuation),
        metrics.clone(),
        Arc::new(AlloyBalancesOracle::new(
            stack.h.maker_provider.clone(),
            *stack.h.aqua.address(),
        )),
        events.clone(),
        block_times,
        clock.clone(),
        ChainId(stack.chain_id),
    ));
    let pools = Arc::new(PoolService::new(
        Arc::clone(&registry),
        Arc::clone(&assets),
        Arc::clone(&valuation),
        metrics,
        Arc::new(AlloyBalancesOracle::new(
            stack.h.maker_provider.clone(),
            *stack.h.aqua.address(),
        )),
        clock.clone(),
    ));
    let guards = Arc::new(StrategyGuard::default());
    let quote = Arc::new(QuoteService::new(
        Arc::clone(&registry),
        Arc::clone(&ledger),
        Arc::clone(&guards),
        Arc::clone(&assets),
        RoutingConfig::new(64, 8, 150_000),
        clock.clone(),
        Arc::clone(&leg_cost),
        Arc::clone(&valuation),
    ));
    let fill_builder: Arc<dyn FillBuilder> = Arc::new(ProtocolFillBuilder::new(
        Arc::new(stack.fill_builder()),
        Some(Arc::new(stack.erc7683_fill_builder())),
    ));
    let swap = Arc::new(SwapService::new(
        Arc::clone(&registry),
        Arc::clone(&ledger),
        Arc::clone(&guards),
        Arc::clone(&trades),
        execution,
        fill_builder,
        Arc::clone(&leg_cost),
        clock.clone(),
        SwapConfig {
            routing: RoutingConfig::new(64, 8, 150_000),
            chain_id: stack.chain_id,
            filler: stack.filler,
            filler_owner: stack.h.maker,
            reservation_ttl_secs: 60,
        },
    ));
    let rebate_authorizer = Arc::new(LocalPolicySigner::new(
        stack.chain_id,
        stack.erc7683_filler,
        stack.policy_signer.clone(),
    ));
    let rebate_store = Arc::new(SqliteRebateStore::new(state_pool.clone()));
    let rebates = Arc::new(RebateService::new(
        RebatePolicy::new(RebatePolicyConfig::new(50, 12_000)),
        Arc::clone(&ledger),
        Arc::clone(&guards),
        rebate_store,
        Arc::new(FillerRebateCallBuilder::new(
            stack.credential,
            rebate_authorizer,
        )),
        RebateMarketData::new(rebate_market, gas, Arc::clone(&assets)),
        RebateServiceConfig::new(Address::ZERO, 150_000, 60),
    ));
    let trade_service = Arc::new(TradeService::new(
        trades,
        Arc::clone(&assets),
        Arc::clone(&valuation),
        Arc::clone(&registry),
    ));
    let history = BinanceHistory::new("http://127.0.0.1:1".to_string(), HashMap::new(), &[])
        .expect("history source");
    let (head, _head_poller) =
        ChainHead::new(stack.h.maker_provider.clone(), Duration::from_secs(60));
    let cosigner_signer = PrivateKeySigner::random();

    (
        AppState {
            config: Arc::new(AppConfig {
                chain_id: stack.chain_id,
                features: Features {
                    faucet: false,
                    earn: false,
                    send_buy: false,
                },
                default_fee_bps: 5,
                networks: vec!["Anvil".to_string()],
                block_explorer_url: "http://localhost".to_string(),
                aqua: *stack.h.aqua.address(),
                app: stack.h.app,
                reactor: stack.reactor,
                permit2: PERMIT2,
                filler: stack.filler,
                erc7683_settler: Some(stack.erc7683_settler),
                erc7683_filler: Some(stack.erc7683_filler),
                erc7683_resolver: None,
                taker_credential: stack.credential,
                cosigner: Address::ZERO,
            }),
            head,
            assets: Arc::clone(&assets),
            pair_history: Arc::new(PairHistoryService::new(Arc::new(history))),
            pools,
            depth: DepthReader::new(Arc::new(DepthService::new(
                Arc::clone(&registry),
                Arc::clone(&ledger),
                guards,
                Arc::clone(&assets),
            ))),
            balances,
            makers,
            quote,
            swap,
            rebates,
            cosigner: Arc::new(ServerCosigner::new(
                PERMIT2,
                stack.reactor,
                stack.chain_id,
                cosigner_signer.clone(),
                stack.filler,
                60,
            )),
            normalizer: Arc::new(UniswapXV2Normalizer::new(
                stack.reactor,
                vec![cosigner_signer.address()],
            )),
            erc7683: Some(Arc::new(Erc7683Normalizer::new(
                ChainId(stack.chain_id),
                PERMIT2,
                stack.erc7683_settler,
                fee_policy,
            ))),
            erc7683_fee_policy: Some(fee_policy),
            trades: trade_service,
            registry,
            registry_store: events,
            valuation,
            quote_log: Arc::new(SqliteQuoteLog::new(state_pool, clock)),
            feed_health: None,
            order_log: None,
        },
        state_dir,
    )
}

async fn exercise_protected_curve(label: &str) {
    let stack = setup().await;
    let h = &stack.h;
    let spec = strategy(h, label);
    let order = spec.order();
    assert_eq!(order.data[0], 0x0e, "{label}: credential opcode");
    assert_eq!(order.data[1], 20, "{label}: credential address length");
    assert_eq!(
        &order.data[2..22],
        stack.credential.as_slice(),
        "{label}: deployed filler credential"
    );
    h.ship(&spec).await;
    let (registry_sync, snapshot, _rp, _rd) = pipeline(h, CHAIN).await;
    registry_sync
        .sync_once(h.latest_block().await)
        .await
        .expect("initial registry sync");
    let (svc, _ld) = ledger(h, &snapshot).await;
    let budgets = budget_source(h, &snapshot);

    // The swapper sells t0 for t1; we source t1 from the maker. Output a tenth of the maker's t1
    // reserve; commit a generous t0 input so there is room to source it profitably.
    let output = spec.ship_hi / U256::from(10u64);
    let input = spec.ship_lo / U256::from(2u64);
    let recipient = address!("dEADbEEF00000000000000000000000000000000");

    // Fund the swapper (taker) with the input token and approve Permit2 to pull it.
    MockERC20::new(h.t0, h.taker_provider.clone())
        .mint(h.taker, input)
        .send()
        .await
        .expect("mint")
        .watch()
        .await
        .expect("mint mined");
    MockERC20::new(h.t0, h.taker_provider.clone())
        .approve(PERMIT2, U256::MAX)
        .send()
        .await
        .expect("approve permit2")
        .watch()
        .await
        .expect("approve permit2 mined");

    // Build + sign the order as the swapper (taker), cosigned by a key we hold; the exclusive filler
    // is our filler *contract* (its address is the reactor's msg.sender).
    let now = h
        .maker_provider
        .get_block(alloy::eips::BlockId::latest())
        .await
        .expect("block")
        .expect("some block")
        .header
        .timestamp;
    let cosigner = PrivateKeySigner::random();
    let cosigner_address = cosigner.address();
    let builder =
        SignedOrderBuilder::new(PERMIT2, stack.chain_id, h.taker_signer.clone(), cosigner);
    let order = OrderSpec {
        reactor: stack.reactor,
        nonce: U256::from(1u64),
        deadline: now + 1000,
        input_token: h.t0,
        input_start: input,
        input_end: input,
        output_token: h.t1,
        output_start: output,
        output_end: output,
        recipient,
        decay_start: now + 10,
        decay_end: now + 100,
        exclusive_filler: stack.filler,
        exclusivity_override_bps: 100,
    };
    let feed = SelfHostedFeed::new(&builder, std::slice::from_ref(&order), now);

    // Ingest: stream → normalize.
    let raws: Vec<RawOrder> = feed.stream().collect().await;
    let intent = UniswapXV2Normalizer::new(stack.reactor, vec![cosigner_address])
        .normalize(&raws[0])
        .expect("normalize");

    // Route the required output against the caps, then reserve the plan.
    let snap = snapshot.load();
    let caps = quote_caps(&snap, &budgets, h.t1, h.t0).await;
    let cfg = RoutingConfig::new(64, 8, 150_000);
    let req = RouteRequest {
        intent: intent.id,
        token_in: h.t0,
        token_out: h.t1,
        amount: output,
        exact_in: false,
    };
    let plan = route(
        RoutingBook::new(&snap, &caps, &GuardSnapshot::default()),
        &req,
        input,
        &cfg,
        U256::ZERO,
        None,
    )
    .plan
    .expect("a routable plan");
    let sources: Vec<ReservationSource> = plan
        .legs
        .iter()
        .map(|l| ReservationSource {
            maker: l.maker,
            strategy_hash: l.strategy_hash,
            token: l.token_out,
            amount: l.amount_out,
        })
        .collect();
    svc.reserve(rid(1), intent.id, sources, 60)
        .await
        .expect("reserve the routed plan");

    // Build the fill calldata and settle it on-chain as the filler's owner.
    let fill = stack
        .fill_builder()
        .build(&intent, &plan, &snap)
        .await
        .expect("fill calldata");
    assert_eq!(fill.target, stack.filler);
    let receipt = h
        .maker_provider
        .send_transaction(
            TransactionRequest::default()
                .to(fill.target)
                .input(TransactionInput::new(fill.calldata)),
        )
        .await
        .expect("send fill")
        .get_receipt()
        .await
        .expect("fill receipt");
    assert!(receipt.status(), "fill reverted");

    // The swapper's recipient received the promised output.
    assert_eq!(balance_of(h, h.t1, recipient).await, output);
    // The reservation is held against the maker's strategy-virtual room.
    let held = AccountKey::StrategyVirtual {
        maker: plan.legs[0].maker,
        strategy_hash: plan.legs[0].strategy_hash,
        token: h.t1,
    };
    assert!(svc.available(&held) < caps.available(&held));

    execute_rebate(&stack, &spec, &registry_sync, &snapshot).await;
}

fn strategy(h: &common::Harness, label: &str) -> StrategySpec {
    h.fx.strategies
        .iter()
        .find(|strategy| strategy.label == label)
        .cloned()
        .unwrap_or_else(|| panic!("missing {label} fixture"))
}

async fn execute_rebate(
    stack: &common::Stack,
    spec: &StrategySpec,
    registry_sync: &solvent_core::registry::RegistrySync,
    snapshot: &Arc<SharedSnapshot>,
) {
    let h = &stack.h;
    registry_sync
        .sync_once(h.latest_block().await)
        .await
        .expect("post-fill registry sync");
    let strategy = snapshot
        .load()
        .strategy(&spec.key(h.maker, h.app))
        .cloned()
        .expect("displaced strategy");
    let (before_lo, before_hi) = h.on_chain_balances(spec).await;
    let amount_out = (before_lo - spec.ship_lo) / U256::from(2u64);
    assert!(!amount_out.is_zero(), "{}: displaced t0", spec.label);
    let amount_in = price(&strategy, h.t1, h.t0, amount_out, false).expect("reverse quote");
    let maker_rebate = (amount_in / U256::from(100u64)).max(U256::from(1u64));
    let trade_id = TradeId(Ulid::new());
    let plan = RebatePlan::new(
        RebateBatchId(keccak256(
            [spec.label.as_bytes(), b"batch".as_slice()].concat(),
        )),
        strategy.key,
        h.t1,
        h.t0,
        amount_in,
        amount_out,
        maker_rebate + U256::from(1u64),
        U256::ZERO,
        maker_rebate,
        U256::from(1u64),
        100,
        vec![RebateAllocation::new(trade_id, maker_rebate)],
    );
    let builder = FillerRebateCallBuilder::new(
        stack.credential,
        Arc::new(LocalPolicySigner::new(
            stack.chain_id,
            stack.filler,
            stack.policy_signer.clone(),
        )),
    );
    let execution = builder
        .build(
            &strategy,
            plan,
            ReservationId(B256::from([0x55; 32])),
            h.latest_block().await + 100,
            1,
        )
        .await
        .expect("rebate calldata");
    let deposit = amount_in + maker_rebate;
    MockERC20::new(h.t1, h.taker_provider.clone())
        .mint(h.taker, deposit)
        .send()
        .await
        .expect("mint rebate input")
        .watch()
        .await
        .expect("rebate input minted");
    MockERC20::new(h.t1, h.taker_provider.clone())
        .approve(stack.filler, deposit)
        .send()
        .await
        .expect("approve rebate")
        .watch()
        .await
        .expect("rebate approval mined");
    let maker_input_before = balance_of(h, h.t1, h.maker).await;
    let executor_output_before = balance_of(h, h.t0, h.taker).await;
    let filler_input_before = balance_of(h, h.t1, stack.filler).await;
    let filler_output_before = balance_of(h, h.t0, stack.filler).await;

    let receipt = h
        .taker_provider
        .send_transaction(
            TransactionRequest::default()
                .to(stack.filler)
                .input(TransactionInput::new(execution.calldata)),
        )
        .await
        .expect("send rebate")
        .get_receipt()
        .await
        .expect("rebate receipt");
    assert!(receipt.status(), "{}: rebate reverted", spec.label);

    assert_eq!(
        balance_of(h, h.t1, h.maker).await,
        maker_input_before + deposit,
        "{}: maker receives curve input and rebate",
        spec.label
    );
    assert_eq!(
        balance_of(h, h.t0, h.taker).await,
        executor_output_before + amount_out,
        "{}: executor receives exact output",
        spec.label
    );
    assert_eq!(
        balance_of(h, h.t1, stack.filler).await,
        filler_input_before,
        "{}: filler retains no rebate input",
        spec.label
    );
    assert_eq!(
        balance_of(h, h.t0, stack.filler).await,
        filler_output_before,
        "{}: filler retains no rebate output",
        spec.label
    );
    let (after_lo, after_hi) = h.on_chain_balances(spec).await;
    assert_eq!(
        after_lo,
        before_lo - amount_out,
        "{}: t0 restored",
        spec.label
    );
    assert_eq!(
        after_hi,
        before_hi + amount_in,
        "{}: t1 restored",
        spec.label
    );
}
