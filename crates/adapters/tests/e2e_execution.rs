//! Live full-loop E2E through the **production execution path**: order → normalize → route →
//! reserve → sim → submit → track → confirm → post the *actual* pulled amount (or, for a stale
//! order, sim-reject → void). The first proof the whole resolve→fill loop holds against the real
//! contracts and a real walletkit `Wallet`. Submission is public here (anvil has no private relay);
//! production defaults to a private relay. Gated on anvil.

mod common;

use std::sync::Arc;

use alloy::primitives::{address, Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::signers::local::PrivateKeySigner;
use futures::StreamExt;

use common::{
    balance_of, budget_source, drive_execution, execution_pool, execution_service, first_supported,
    ledger, quote_caps, rid, setup, synced, MockERC20, Stack, PERMIT2,
};
use solvent_adapters::ingest::uniswapx::{
    OrderSpec, SelfHostedFeed, SignedOrderBuilder, UniswapXV2Normalizer,
};
use solvent_core::deps::ingest::{FillBuilder, Normalizer, OrderFeed};
use solvent_core::ledger::{AvailableSnapshot, LedgerService};
use solvent_core::primitives::execution::{FillOutcome, FillTx, PendingFill};
use solvent_core::primitives::ingest::{Intent, RawOrder};
use solvent_core::primitives::ledger::{AccountKey, ReservationSource};
use solvent_core::primitives::routing::{RoutePlan, RouteRequest, RoutingConfig};
use solvent_core::registry::SharedSnapshot;
use solvent_core::routing::{route, GuardSnapshot, RoutingBook};

const RECIPIENT: Address = address!("dEADbEEF00000000000000000000000000000000");

/// Build a signed self-hosted order, normalize it, route the required output, reserve the plan, and
/// return the fill calldata. `stale` makes the order's deadline already-past so its on-chain fill
/// (and thus its simulation) reverts.
async fn reserve_order(
    stack: &Stack,
    snapshot: &Arc<SharedSnapshot>,
    svc: &LedgerService,
    output: U256,
    input: U256,
    stale: bool,
) -> (Intent, RoutePlan, Bytes, AvailableSnapshot) {
    let h = &stack.h;
    let budgets = budget_source(h, snapshot);

    // Fund + approve the swapper so a valid fill can pull the input.
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

    let now = h
        .maker_provider
        .get_block(alloy::eips::BlockId::latest())
        .await
        .expect("block")
        .expect("some block")
        .header
        .timestamp;
    let (deadline, decay_start, decay_end) = if stale {
        (now - 1, now - 100, now - 50)
    } else {
        (now + 1000, now + 10, now + 100)
    };
    let cosigner = PrivateKeySigner::random();
    let builder =
        SignedOrderBuilder::new(PERMIT2, stack.chain_id, h.taker_signer.clone(), cosigner);
    let order = OrderSpec {
        reactor: stack.reactor,
        nonce: U256::from(1u64),
        deadline,
        input_token: h.t0,
        input_start: input,
        input_end: input,
        output_token: h.t1,
        output_start: output,
        output_end: output,
        recipient: RECIPIENT,
        decay_start,
        decay_end,
        exclusive_filler: stack.filler,
    };
    let feed = SelfHostedFeed::new(&builder, std::slice::from_ref(&order), now);
    let raws: Vec<RawOrder> = feed.stream().collect().await;
    let intent = UniswapXV2Normalizer.normalize(&raws[0]).expect("normalize");

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
    let fill = stack
        .fill_builder()
        .build(&intent, &plan, &snap)
        .await
        .expect("fill calldata");
    (intent, plan, fill.calldata, caps)
}

#[tokio::test]
async fn e2e_fill_confirms_and_posts_the_actual_pulled_amount() {
    if common::skip_without_anvil() {
        return;
    }
    let stack = setup().await;
    let spec = first_supported(&stack.h);
    stack.h.ship(&spec).await;
    let (snapshot, _rp, _rd) = synced(&stack.h).await;
    let (svc_ledger, _ld) = ledger(&stack.h, &snapshot).await;
    let led = Arc::new(svc_ledger);

    let output = spec.ship_hi / U256::from(10u64);
    let input = spec.ship_lo / U256::from(2u64);
    let (intent, plan, calldata, caps) =
        reserve_order(&stack, &snapshot, &led, output, input, false).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let pool = execution_pool(dir.path()).await;
    let svc = execution_service(&stack.h, led.clone(), pool, &dir.path().join("wallet.redb"));
    let fill = PendingFill::new(
        FillTx::new(
            intent.id,
            stack.chain_id,
            stack.h.maker,
            stack.filler,
            calldata,
        ),
        rid(1),
    );
    assert!(matches!(
        svc.fill(fill).await.expect("fill"),
        FillOutcome::Submitted { .. }
    ));

    drive_execution(&svc, &stack.h).await;
    assert_eq!(svc.pending().await.expect("pending"), 0, "the fill settled");

    // The swapper's recipient received the promised output on-chain.
    assert_eq!(balance_of(&stack.h, stack.h.t1, RECIPIENT).await, output);
    // The reservation was *posted* the actual pulled amount: available stays reduced by the
    // consumed output (a void would have released it back to the full cap).
    let held = AccountKey::StrategyVirtual {
        maker: plan.legs[0].maker,
        strategy_hash: plan.legs[0].strategy_hash,
        token: stack.h.t1,
    };
    assert_eq!(
        led.available(&held),
        caps.available(&held) - output,
        "posted the pulled amount, not released"
    );
}

#[tokio::test]
async fn e2e_stale_order_is_rejected_by_sim_and_voided() {
    if common::skip_without_anvil() {
        return;
    }
    let stack = setup().await;
    let spec = first_supported(&stack.h);
    stack.h.ship(&spec).await;
    let (snapshot, _rp, _rd) = synced(&stack.h).await;
    let (svc_ledger, _ld) = ledger(&stack.h, &snapshot).await;
    let led = Arc::new(svc_ledger);

    let output = spec.ship_hi / U256::from(10u64);
    let input = spec.ship_lo / U256::from(2u64);
    let (intent, plan, calldata, caps) =
        reserve_order(&stack, &snapshot, &led, output, input, true).await;
    let held = AccountKey::StrategyVirtual {
        maker: plan.legs[0].maker,
        strategy_hash: plan.legs[0].strategy_hash,
        token: stack.h.t1,
    };
    assert_eq!(
        led.available(&held),
        caps.available(&held) - output,
        "held on reserve"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let pool = execution_pool(dir.path()).await;
    let svc = execution_service(&stack.h, led.clone(), pool, &dir.path().join("wallet.redb"));
    let fill = PendingFill::new(
        FillTx::new(
            intent.id,
            stack.chain_id,
            stack.h.maker,
            stack.filler,
            calldata,
        ),
        rid(1),
    );

    // The stale order's fill reverts in simulation → dropped before any nonce, reservation voided.
    assert!(matches!(
        svc.fill(fill).await.expect("fill"),
        FillOutcome::Rejected { .. }
    ));
    assert_eq!(
        svc.pending().await.expect("pending"),
        0,
        "nothing submitted"
    );
    assert_eq!(
        led.available(&held),
        caps.available(&held),
        "the hold was released by the void"
    );
}

#[tokio::test]
async fn e2e_recovers_an_in_flight_fill_after_restart() {
    if common::skip_without_anvil() {
        return;
    }
    let stack = setup().await;
    let spec = first_supported(&stack.h);
    stack.h.ship(&spec).await;
    let (snapshot, _rp, _rd) = synced(&stack.h).await;
    let (svc_ledger, _ld) = ledger(&stack.h, &snapshot).await;
    let led = Arc::new(svc_ledger);

    let output = spec.ship_hi / U256::from(10u64);
    let input = spec.ship_lo / U256::from(2u64);
    let (intent, plan, calldata, caps) =
        reserve_order(&stack, &snapshot, &led, output, input, false).await;

    // Durable execution stores that outlive the "crashed" service instance.
    let dir = tempfile::tempdir().expect("tempdir");
    let pool = execution_pool(dir.path()).await;
    let redb = dir.path().join("wallet.redb");

    // First instance: submit the fill, then drop everything (a crash) WITHOUT reconciling.
    {
        let svc = execution_service(&stack.h, led.clone(), pool.clone(), &redb);
        assert!(matches!(
            svc.fill(PendingFill::new(
                FillTx::new(
                    intent.id,
                    stack.chain_id,
                    stack.h.maker,
                    stack.filler,
                    calldata,
                ),
                rid(1),
            ))
            .await
            .expect("fill"),
            FillOutcome::Submitted { .. }
        ));
        assert_eq!(
            svc.pending().await.expect("pending"),
            1,
            "one fill in flight"
        );
    }

    // Fresh instance over the SAME durable stores — recovery is emergent from reconcile alone, with
    // no explicit recovery step and no in-memory state carried over.
    let svc = execution_service(&stack.h, led.clone(), pool.clone(), &redb);
    assert_eq!(
        svc.pending().await.expect("pending"),
        1,
        "the in-flight fill survived the restart"
    );

    drive_execution(&svc, &stack.h).await;
    assert_eq!(
        svc.pending().await.expect("pending"),
        0,
        "the recovered fill settled"
    );

    assert_eq!(balance_of(&stack.h, stack.h.t1, RECIPIENT).await, output);
    let held = AccountKey::StrategyVirtual {
        maker: plan.legs[0].maker,
        strategy_hash: plan.legs[0].strategy_hash,
        token: stack.h.t1,
    };
    assert_eq!(
        led.available(&held),
        caps.available(&held) - output,
        "the recovered fill posted the pulled amount"
    );
}
