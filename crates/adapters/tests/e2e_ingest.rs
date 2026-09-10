//! Live full-loop E2E: a self-hosted UniswapX order → normalize → route → reserve → **fill on-chain**
//! through the deployed reactor + `UniswapXAquaFiller` via a direct `fill()` call, sourcing the
//! output from a shipped Aqua maker. Proves the ingest→fill path against the real contracts; the
//! sibling `e2e_execution` drives the same loop through the production execution service. Gated on anvil.

mod common;

use std::sync::Arc;

use alloy::primitives::{address, keccak256, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::{TransactionInput, TransactionRequest};
use alloy::signers::local::PrivateKeySigner;
use futures::StreamExt;
use ulid::Ulid;

use common::{
    balance_of, budget_source, ledger, pipeline, quote_caps, rid, setup, MockERC20, StrategySpec,
    CHAIN, PERMIT2,
};
use solvent_adapters::execution::LocalPolicySigner;
use solvent_adapters::ingest::uniswapx::{
    OrderSpec, SelfHostedFeed, SignedOrderBuilder, UniswapXV2Normalizer,
};
use solvent_adapters::rebate::FillerRebateCallBuilder;
use solvent_core::deps::ingest::{FillBuilder, Normalizer, OrderFeed};
use solvent_core::deps::rebate::RebateCallBuilder;
use solvent_core::primitives::ingest::RawOrder;
use solvent_core::primitives::ledger::{AccountKey, ReservationSource};
use solvent_core::primitives::rebate::{RebateAllocation, RebatePlan};
use solvent_core::primitives::routing::{RouteRequest, RoutingConfig};
use solvent_core::primitives::trade::TradeId;
use solvent_core::primitives::{RebateBatchId, ReservationId};
use solvent_core::registry::price;
use solvent_core::registry::SharedSnapshot;
use solvent_core::routing::{route, GuardSnapshot, RoutingBook};

#[tokio::test]
async fn e2e_protected_curves_support_user_fills_and_rebates() {
    if common::skip_without_anvil() {
        return;
    }
    for label in ["xyc", "concentrate", "pegged_18_18"] {
        exercise_protected_curve(label).await;
    }
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
    };
    let feed = SelfHostedFeed::new(&builder, std::slice::from_ref(&order), now);

    // Ingest: stream → normalize.
    let raws: Vec<RawOrder> = feed.stream().collect().await;
    let intent = UniswapXV2Normalizer.normalize(&raws[0]).expect("normalize");

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
    let calldata = stack
        .fill_builder()
        .build(&intent, &plan, &snap)
        .await
        .expect("fill calldata");
    let receipt = h
        .maker_provider
        .send_transaction(
            TransactionRequest::default()
                .to(stack.filler)
                .input(TransactionInput::new(calldata)),
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
