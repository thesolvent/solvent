//! Live full-loop E2E: a self-hosted UniswapX order → normalize → route → reserve → **fill on-chain**
//! through the deployed reactor + `UniswapXAquaFiller` via a direct `fill()` call, sourcing the
//! output from a shipped Aqua maker. Proves the ingest→fill path against the real contracts; the
//! sibling `e2e_execution` drives the same loop through the production execution service. Gated on anvil.

mod common;

use alloy::primitives::{address, U256};
use alloy::providers::Provider;
use alloy::rpc::types::{TransactionInput, TransactionRequest};
use alloy::signers::local::PrivateKeySigner;
use futures::StreamExt;

use common::{
    balance_of, budget_source, first_supported, ledger, quote_caps, rid, setup, synced, MockERC20,
    PERMIT2,
};
use solvent_adapters::ingest::uniswapx::{
    OrderSpec, SelfHostedFeed, SignedOrderBuilder, UniswapXV2Normalizer,
};
use solvent_core::deps::ingest::{FillBuilder, Normalizer, OrderFeed};
use solvent_core::primitives::ingest::RawOrder;
use solvent_core::primitives::ledger::{AccountKey, ReservationSource};
use solvent_core::primitives::routing::{RouteRequest, RoutingConfig};
use solvent_core::routing::route;

#[tokio::test]
async fn e2e_self_hosted_order_fills_on_chain() {
    if common::skip_without_anvil() {
        return;
    }
    let stack = setup().await;
    let h = &stack.h;
    let spec = first_supported(h);
    h.ship(&spec).await;
    let (snapshot, _rp, _rd) = synced(h).await;
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
    let plan = route(&snap, &caps, &req, input, &cfg, U256::ZERO, None).expect("a routable plan");
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
}
