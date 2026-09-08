//! The phase's headline: one engine, two protocols. A UniswapX order and an ERC-7683 order are fanned
//! into a single unmodified `IngestPipeline`, routed and reserved against the **same maker balance**,
//! and settled on-chain through their own filler contracts. Everything between `Intent` and the fill
//! calldata — routing, ledger, execution — is shared and untouched. Gated on anvil.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use alloy::network::TransactionBuilder;
use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolValue;
use futures::StreamExt;
use tokio::sync::mpsc;

use common::{
    balance_of, budget_source, first_supported, ledger, quote_caps, rid, setup, synced, Harness,
    MockERC20, Stack, PERMIT2,
};
use solvent_adapters::ingest::{erc7683, uniswapx};
use solvent_core::deps::ingest::{FillBuilder, Normalizer, OrderFeed};
use solvent_core::deps::ledger::Clock;
use solvent_core::ingest::IngestPipeline;
use solvent_core::primitives::ingest::{Intent, ProtocolId, RawOrder};
use solvent_core::primitives::ledger::ReservationSource;
use solvent_core::primitives::routing::{RoutePlan, RouteRequest, RoutingConfig};
use solvent_core::primitives::ChainId;
use solvent_core::routing::route;

// The 7683 contracts' ABIs pull in `ISwapVM` (the filler's `SourceSwap.order`), which the router's
// `sol!` in `common` already defines at module scope — so they live in their own module.
mod erc7683_abi {
    alloy::sol!(
        #[sol(rpc)]
        #[allow(clippy::too_many_arguments)]
        SameChainSettler,
        "tests/fixtures/artifacts/SameChainSettler.json"
    );
}
use erc7683_abi::SameChainSettler;

/// The filler's ABI references `ISwapVM`'s `MakerTraits` value type, which alloy's json-abi codegen
/// cannot resolve — and we only ever deploy it, so take the bytecode straight from the artifact.
fn deploy_from_artifact(artifact: &str, ctor_args: Bytes) -> Bytes {
    let v: serde_json::Value = serde_json::from_str(artifact).expect("artifact json");
    let hex = v["bytecode"]["object"].as_str().expect("bytecode object");
    let code: Bytes = hex.parse().expect("bytecode hex");
    [code.as_ref(), ctor_args.as_ref()].concat().into()
}

struct FixedClock(u64);
impl Clock for FixedClock {
    fn now_unix(&self) -> u64 {
        self.0
    }
}

/// The two protocol deployments the engine fills through.
struct Fillers {
    settler: Address,
    erc7683: Address,
}

async fn deploy_erc7683(h: &Harness) -> Fillers {
    let settler = SameChainSettler::deploy(h.maker_provider.clone(), PERMIT2)
        .await
        .expect("deploy settler");
    let code = deploy_from_artifact(
        include_str!("fixtures/artifacts/Erc7683AquaFiller.json"),
        Bytes::from(h.maker.abi_encode()),
    );
    let receipt = h
        .maker_provider
        .send_transaction(TransactionRequest::default().with_deploy_code(code))
        .await
        .expect("deploy 7683 filler")
        .get_receipt()
        .await
        .expect("deploy receipt");
    Fillers {
        settler: *settler.address(),
        erc7683: receipt.contract_address.expect("filler address"),
    }
}

/// Mint the taker their input and let Permit2 move it — the swapper side of both protocols.
async fn fund_taker(h: &Harness, amount: U256) {
    MockERC20::new(h.t0, h.taker_provider.clone())
        .mint(h.taker, amount)
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
}

async fn now_ts(h: &Harness) -> u64 {
    h.maker_provider
        .get_block(alloy::eips::BlockId::latest())
        .await
        .expect("block")
        .expect("some block")
        .header
        .timestamp
}

/// A signed ERC-7683 order, opened on-chain so its input sits in the settler's escrow.
async fn open_erc7683(
    h: &Harness,
    f: &Fillers,
    chain_id: u64,
    input: U256,
    output: U256,
    now: u64,
) -> RawOrder {
    let builder = erc7683::SignedOrderBuilder::new(PERMIT2, chain_id, h.taker_signer.clone());
    let spec = erc7683::OrderSpec {
        settler: f.settler,
        nonce: U256::from(now),
        open_deadline: u32::try_from(now + 600).expect("fits"),
        fill_deadline: u32::try_from(now + 900).expect("fits"),
        input_token: h.t0,
        input_amount: input,
        output_token: h.t1,
        output_amount: output,
        recipient: h.taker,
        // Only our filler may fill inside the window — the same shape UniswapX exclusivity takes.
        exclusive_filler: f.erc7683,
        exclusivity_ends: u32::try_from(now + 900).expect("fits"),
    };
    let feed = erc7683::SelfHostedFeed::new(&builder, std::slice::from_ref(&spec), now);
    let raw = feed.stream().collect::<Vec<_>>().await.remove(0);

    let order =
        SameChainSettler::GaslessCrossChainOrder::abi_decode(&raw.payload).expect("round-trips");
    SameChainSettler::new(f.settler, h.taker_provider.clone())
        .openFor(order, raw.signature.clone(), Bytes::new())
        .send()
        .await
        .expect("openFor")
        .watch()
        .await
        .expect("openFor mined");

    raw
}

fn uniswapx_order(stack: &Stack, input: U256, output: U256, now: u64) -> RawOrder {
    let h = &stack.h;
    let builder = uniswapx::SignedOrderBuilder::new(
        PERMIT2,
        stack.chain_id,
        h.taker_signer.clone(),
        h.maker_signer.clone(),
    );
    let spec = uniswapx::OrderSpec {
        reactor: stack.reactor,
        nonce: U256::from(now + 1),
        deadline: now + 900,
        input_token: h.t0,
        input_start: input,
        input_end: input,
        output_token: h.t1,
        output_start: output,
        output_end: output,
        recipient: h.taker,
        decay_start: now,
        decay_end: now + 900,
        exclusive_filler: stack.filler,
    };
    builder.build(&spec, now)
}

/// Fan both feeds into one pipeline with both normalizers — the pipeline itself is unmodified.
async fn ingest_both(raws: Vec<RawOrder>, now: u64, chain: ChainId) -> Vec<Intent> {
    struct Once(Vec<RawOrder>);
    impl OrderFeed for Once {
        fn stream(&self) -> futures::stream::BoxStream<'static, RawOrder> {
            futures::stream::iter(self.0.clone()).boxed()
        }
    }

    let mut normalizers: BTreeMap<ProtocolId, Arc<dyn Normalizer>> = BTreeMap::new();
    normalizers.insert(
        ProtocolId::UniswapXV2,
        Arc::new(uniswapx::UniswapXV2Normalizer),
    );
    normalizers.insert(ProtocolId::Erc7683, Arc::new(erc7683::Erc7683Normalizer));

    let pipeline = IngestPipeline::new(
        normalizers,
        Duration::from_secs(60),
        Arc::new(FixedClock(now)),
        BTreeSet::from([chain]),
    );

    let (tx, mut rx) = mpsc::channel(16);
    pipeline
        .run(vec![Arc::new(Once(raws)) as Arc<dyn OrderFeed>], tx)
        .await;
    let mut out = Vec::new();
    while let Some(i) = rx.recv().await {
        out.push(i);
    }
    out
}

/// The protocol → fill-builder dispatch. Deliberately a call-site map, not a port: the composition
/// root is where it belongs once it exists, and nothing else needs it yet.
fn fill_calldata(
    intent: &Intent,
    plan: &RoutePlan,
    snap: &solvent_core::primitives::registry::Snapshot,
    app: Address,
) -> Bytes {
    let builders: BTreeMap<ProtocolId, Arc<dyn FillBuilder>> = BTreeMap::from([
        (
            ProtocolId::UniswapXV2,
            Arc::new(uniswapx::UniswapXFillBuilder::new(app)) as Arc<dyn FillBuilder>,
        ),
        (
            ProtocolId::Erc7683,
            Arc::new(erc7683::Erc7683FillBuilder::new(app)) as Arc<dyn FillBuilder>,
        ),
    ]);
    builders
        .get(&intent.protocol)
        .expect("a builder for every ingested protocol")
        .build(intent, plan, snap)
        .expect("fill calldata")
}

async fn send_fill(h: &Harness, to: Address, calldata: Bytes) {
    let tx = TransactionRequest::default()
        .with_to(to)
        .with_input(calldata);
    h.maker_provider
        .send_transaction(tx)
        .await
        .expect("submit fill")
        .watch()
        .await
        .expect("fill mined");
}

#[tokio::test]
async fn erc7683_order_settles_end_to_end_like_uniswapx() {
    if common::skip_without_anvil() {
        return;
    }
    let stack = setup().await;
    let h = &stack.h;
    let f = deploy_erc7683(h).await;

    let spec = first_supported(h);
    h.ship(&spec).await;
    let (snapshot, _pool, _dir) = synced(h).await;

    let output = common::e18(1);
    let input = common::e18(3100);
    fund_taker(h, input).await;
    let now = now_ts(h).await;

    let raw = open_erc7683(h, &f, stack.chain_id, input, output, now).await;
    assert_eq!(
        balance_of(h, h.t0, f.settler).await,
        input,
        "openFor escrowed the swapper's input"
    );

    let intents = ingest_both(vec![raw], now, common::CHAIN).await;
    assert_eq!(intents.len(), 1);
    let intent = &intents[0];
    assert_eq!(intent.protocol, ProtocolId::Erc7683);

    // Route + reserve are protocol-blind: the same calls the UniswapX path makes.
    let (svc, _led_dir) = ledger(h, &snapshot).await;
    let budgets = budget_source(h, &snapshot);
    let snap = snapshot.load();
    let caps = quote_caps(&snap, &budgets, h.t1, h.t0).await;
    let req = RouteRequest {
        intent: intent.id,
        token_in: h.t0,
        token_out: h.t1,
        amount: output,
        exact_in: false,
    };
    let plan = route(
        &snap,
        &caps,
        &req,
        input,
        &RoutingConfig::new(64, 8, 150_000),
        U256::ZERO,
        None,
    )
    .expect("a routable plan");
    svc.reserve(rid(1), intent.id, sources_of(&plan), 60)
        .await
        .expect("reserve");

    let taker_out_before = balance_of(h, h.t1, h.taker).await;
    let calldata = fill_calldata(intent, &plan, &snap, h.app);
    send_fill(h, f.erc7683, calldata).await;

    assert_eq!(
        balance_of(h, h.t1, h.taker).await - taker_out_before,
        output,
        "the swapper received the output"
    );
    assert_eq!(
        balance_of(h, h.t0, f.settler).await,
        U256::ZERO,
        "the escrow was released to the filler"
    );
    assert_eq!(
        balance_of(h, h.t1, f.erc7683).await,
        U256::ZERO,
        "zero inventory: the filler kept no output"
    );
    assert!(
        balance_of(h, h.t0, f.erc7683).await > U256::ZERO,
        "the filler kept the spread"
    );
}

fn sources_of(plan: &RoutePlan) -> Vec<ReservationSource> {
    plan.legs
        .iter()
        .map(|l| ReservationSource {
            maker: l.maker,
            strategy_hash: l.strategy_hash,
            token: l.token_out,
            amount: l.amount_out,
        })
        .collect()
}

/// The maker's binding ceiling on the output token: the ledger admits against
/// `min(wallet budget, strategy virtual)`, so contention is sized off the smaller of the two.
fn maker_ceiling(caps: &solvent_core::ledger::AvailableSnapshot) -> U256 {
    caps.0.values().copied().min().expect("a priced maker")
}

/// The phase's headline. Two protocols, one engine, one maker balance: the reservation ledger sees
/// only `Intent`s, so it arbitrates *across* protocols. The loser is declined off-chain — saving the
/// gas of a fill that Aqua would have reverted anyway — and fills once the winner settles.
#[tokio::test]
async fn both_protocols_contend_for_one_maker_balance() {
    if common::skip_without_anvil() {
        return;
    }
    let stack = setup().await;
    let h = &stack.h;
    let f = deploy_erc7683(h).await;

    let spec = first_supported(h);
    h.ship(&spec).await;
    let (snapshot, _pool, _dir) = synced(h).await;

    let (svc, _led_dir) = ledger(h, &snapshot).await;
    let budgets = budget_source(h, &snapshot);
    let snap = snapshot.load();
    let caps = quote_caps(&snap, &budgets, h.t1, h.t0).await;

    // Each order wants 60% of the maker's ceiling, so the two together cannot both be promised.
    let output = maker_ceiling(&caps) * U256::from(6u64) / U256::from(10u64);
    assert!(
        output > U256::ZERO,
        "the maker must have room to contend for"
    );
    let input = common::e18(3_000_000);

    fund_taker(h, input * U256::from(2u64)).await;
    let now = now_ts(h).await;

    let raws = vec![
        open_erc7683(h, &f, stack.chain_id, input, output, now).await,
        uniswapx_order(&stack, input, output, now),
    ];
    let intents = ingest_both(raws, now, common::CHAIN).await;
    assert_eq!(intents.len(), 2, "one pipeline admitted both protocols");

    // Routing is protocol-blind and works off chain state, not the ledger's leases — so *both*
    // orders route successfully. The ledger is the thing that says no.
    let mut plans = Vec::new();
    for intent in &intents {
        let req = RouteRequest {
            intent: intent.id,
            token_in: h.t0,
            token_out: h.t1,
            amount: output,
            exact_in: false,
        };
        let plan = route(
            &snap,
            &caps,
            &req,
            input,
            &RoutingConfig::new(64, 8, 150_000),
            U256::ZERO,
            None,
        )
        .expect("both protocols route against the same maker");
        plans.push(plan);
    }

    let granted = svc
        .reserve(rid(1), intents[0].id, sources_of(&plans[0]), 60)
        .await;
    assert!(granted.is_ok(), "the first intent is promised");

    let declined = svc
        .reserve(rid(2), intents[1].id, sources_of(&plans[1]), 60)
        .await;
    assert!(
        declined.is_err(),
        "the second intent must be declined off-chain — one maker balance cannot back both, \
         and the ledger does not care which protocol asked"
    );

    // The granted one settles on-chain through its own protocol's filler.
    let target = match intents[0].protocol {
        ProtocolId::Erc7683 => f.erc7683,
        _ => stack.filler,
    };
    let taker_out_before = balance_of(h, h.t1, h.taker).await;
    let calldata = fill_calldata(&intents[0], &plans[0], &snap, h.app);
    send_fill(h, target, calldata).await;
    assert_eq!(
        balance_of(h, h.t1, h.taker).await - taker_out_before,
        output,
        "the winner filled"
    );
}
