//! Live full-arc E2E tying the whole recapture thesis together against the real reactor + filler +
//! Aqua/SwapVM: forward flow imbalances a maker's pool → a reverse buy internalizes into that maker
//! and fills on chain → the *actual* fill legs drive the recapture credit → the payout worker rebates
//! the maker as a real ERC-20 transfer. The fill helpers mirror `e2e_execution` so that test stays
//! independent; the internalization *selection* over an alternative maker is covered hermetically in
//! `solvent-core`. Gated on anvil.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use alloy::primitives::{address, Address, Bytes, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::Filter;
use alloy::signers::local::PrivateKeySigner;
use async_trait::async_trait;
use futures::StreamExt;
use rust_decimal::Decimal;

use common::{
    balance_of, budget_source, first_supported, ledger, quote_caps, rid, setup, setup_attached,
    synced, Harness, MockERC20, Stack, PERMIT2,
};
use solvent_adapters::execution::{AquaSettlementReader, SqliteFillStore, WalletkitExecutor};
use solvent_adapters::ingest::uniswapx::{
    OrderSpec, SelfHostedFeed, SignedOrderBuilder, UniswapXFillBuilder, UniswapXV2Normalizer,
};
use solvent_adapters::recapture::{AlloyRebatePayer, AquaSettledLegsReader, SqliteRecaptureStore};
use solvent_core::deps::ingest::{FillBuilder, Normalizer, OrderFeed};
use solvent_core::deps::recapture::RecaptureStore;
use solvent_core::deps::routing::{PriceOracle, PriceOracleError};
use solvent_core::execution::ExecutionService;
use solvent_core::ledger::{AvailableSnapshot, LedgerService};
use solvent_core::primitives::execution::{
    FillOutcome, FillTx, PendingFill, Settled, SettledOutcome,
};
use solvent_core::primitives::ingest::{Intent, RawOrder};
use solvent_core::primitives::ledger::ReservationSource;
use solvent_core::primitives::recapture::RecapturePolicy;
use solvent_core::primitives::routing::{RoutePlan, RouteRequest, RoutingConfig};
use solvent_core::primitives::{Bps, MakerId, Usd, UsdPrice};
use solvent_core::recapture::{PayoutService, RecaptureService};
use solvent_core::registry::SharedSnapshot;
use solvent_core::routing::route;
use sqlx::sqlite::SqlitePoolOptions;
use walletkit::adapters::policy::{AllowAll, DefaultPolicyEngine};
use walletkit::adapters::{LocalSigner, SystemClock, Transport};
use walletkit::core::deps::SubmissionOpts;
use walletkit::Wallet;

const RECIPIENT: Address = address!("dEADbEEF00000000000000000000000000000000");

/// A fair-value oracle for recapture: `token1` priced in `token0` at the pool's *pre-imbalance*
/// ratio, so a maker that later sold `token1` cheap shows a positive LVR. Both tokens are 18-dp
/// MockERC20s.
struct FairOracle {
    token0: Address,
    token1: Address,
    price_token1_in_token0: Decimal,
}

#[async_trait]
impl PriceOracle for FairOracle {
    async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError> {
        if token == self.token0 {
            Ok(UsdPrice(Decimal::ONE))
        } else if token == self.token1 {
            Ok(UsdPrice(self.price_token1_in_token0))
        } else {
            Err(PriceOracleError::NotFound(token))
        }
    }
}

fn to_decimal(value: U256) -> Decimal {
    value
        .to_string()
        .parse()
        .expect("U256 within Decimal range")
}

/// Build a signed self-hosted `t0 → t1` order, normalize it, route the output, reserve the plan, and
/// return the fill calldata (mirrors `e2e_execution::reserve_order`, always live).
async fn reserve_order(
    stack: &Stack,
    snapshot: &Arc<SharedSnapshot>,
    svc: &LedgerService,
    output: U256,
    input: U256,
) -> (Intent, RoutePlan, Bytes, AvailableSnapshot) {
    let h = &stack.h;
    let budgets = budget_source(h, snapshot);

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
        recipient: RECIPIENT,
        decay_start: now + 10,
        decay_end: now + 100,
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
    let calldata = UniswapXFillBuilder::new(h.app)
        .build(&intent, &plan, &snap)
        .expect("fill calldata");
    (intent, plan, calldata, caps)
}

async fn execution_service(h: &Harness, led: Arc<LedgerService>) -> ExecutionService {
    let key_hex = format!("0x{}", alloy::hex::encode(h.maker_signer.to_bytes()));
    let signer = LocalSigner::from_private_key(&key_hex).expect("local signer");
    let policy = DefaultPolicyEngine::new(vec![Box::new(AllowAll)], Arc::new(SystemClock));
    let transport = Transport::url(h.endpoint.parse().expect("endpoint url")).expect("transport");
    let wallet = Wallet::builder(Arc::new(transport), Arc::new(signer), Arc::new(policy))
        .confirmations(1)
        .bump_timeout(0)
        .build();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("fill store sqlite");
    let fills = SqliteFillStore::new(pool);
    fills.migrate().await.expect("migrate the fill store");
    let exec = Arc::new(WalletkitExecutor::new(
        wallet,
        SubmissionOpts::public(),
        Arc::new(fills),
    ));
    let settlement = Arc::new(AquaSettlementReader::new(
        Arc::new(h.maker_provider.clone()),
        *h.aqua.address(),
    ));
    ExecutionService::new(exec.clone(), exec, settlement, led)
}

async fn drive(svc: &ExecutionService, h: &Harness) -> Vec<Settled> {
    let mut confirmed = Vec::new();
    for _ in 0..10 {
        if svc.pending().await.expect("pending") == 0 {
            break;
        }
        let _: () = h
            .maker_provider
            .raw_request("anvil_mine".into(), (2u64,))
            .await
            .expect("anvil_mine");
        confirmed.extend(svc.reconcile().await.expect("reconcile"));
    }
    confirmed
}

/// A base-unit (18-dp) amount as whole tokens, for the walkthrough trace.
fn whole(amount: U256) -> Decimal {
    to_decimal(amount) / to_decimal(U256::from(10u64).pow(U256::from(18u64)))
}

/// The most recent ERC-20 `Transfer` of `token` to `who` at or after `from_block` — the rebate's
/// on-chain tx, read back so the walkthrough can link it in the explorer.
async fn transfer_tx(h: &Harness, token: Address, who: Address, from_block: u64) -> Option<B256> {
    let filter = Filter::new()
        .address(token)
        .from_block(from_block)
        .event("Transfer(address,address,uint256)")
        .topic2(who.into_word());
    h.maker_provider
        .get_logs(&filter)
        .await
        .expect("transfer logs")
        .last()
        .and_then(|log| log.transaction_hash)
}

/// The whole recapture thesis, one run: ship a maker → imbalance its pool with forward flow → a
/// reverse counter-intent internalizes into that maker and fills on chain → the real fill legs drive
/// the recapture credit → the payout worker rebates the maker as a real ERC-20 transfer. Asserts
/// every hop and prints a walkthrough trace; given an explorer base URL, links the on-chain steps.
async fn run_full_arc(stack: &Stack, otterscan: Option<&str>) {
    let h = &stack.h;
    let spec = first_supported(h);
    h.ship(&spec).await;

    // Fair mid = the pool's pre-imbalance reserve ratio (t0 per t1).
    let (t0_pre, t1_pre) = h.on_chain_balances(&spec).await;
    let fair_price_t1 = to_decimal(t0_pre) / to_decimal(t1_pre);

    // Forward flow: takers sell t1 for t0 against the maker, leaving its pool t1-heavy (t1 cheap).
    for _ in 0..4 {
        h.swap(&spec, h.t1, h.t0, t1_pre / U256::from(20u64)).await;
    }
    let (t0_post, t1_post) = h.on_chain_balances(&spec).await;
    let cheap_price_t1 = to_decimal(t0_post) / to_decimal(t1_post);

    let (snapshot, _rp, _rd) = synced(h).await;
    let (svc_ledger, _ld) = ledger(h, &snapshot).await;
    let led = Arc::new(svc_ledger);

    // Reverse buy: pay t0, receive t1 — internalizes into the now-cheap maker and fills on chain.
    let output = spec.ship_hi / U256::from(10u64);
    let input = spec.ship_lo / U256::from(2u64);
    let (intent, plan, calldata, _caps) =
        reserve_order(stack, &snapshot, &led, output, input).await;
    assert!(
        plan.legs.iter().all(|l| l.maker == MakerId(h.maker)),
        "the reverse buy sourced from the imbalanced maker"
    );

    let svc = execution_service(h, led.clone()).await;
    let fill = PendingFill::new(
        FillTx::new(intent.id, stack.chain_id, h.maker, stack.filler, calldata),
        rid(1),
    );
    assert!(matches!(
        svc.fill(fill).await.expect("fill"),
        FillOutcome::Submitted { .. }
    ));
    let fill_tx = drive(&svc, h)
        .await
        .into_iter()
        .find(|s| s.intent == intent.id)
        .and_then(|s| match s.outcome {
            SettledOutcome::Confirmed { tx, .. } => Some(tx),
            _ => None,
        });
    assert_eq!(
        svc.pending().await.expect("pending"),
        0,
        "the reverse fill settled"
    );
    assert!(
        plan.expected_profit > U256::ZERO,
        "resolver spread is positive"
    );

    // Recapture: value the real fill's legs against the fair mid, accrue to the durable store.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite");
    let store = Arc::new(SqliteRecaptureStore::new(pool));
    store.migrate().await.expect("migrate");
    let oracle = Arc::new(FairOracle {
        token0: h.t0,
        token1: h.t1,
        price_token1_in_token0: fair_price_t1,
    });
    let policy = RecapturePolicy::new(
        Bps(Decimal::from(8000u32)),
        Bps(Decimal::from(10000u32)),
        Usd(Decimal::ZERO),
    );
    let settled_legs = Arc::new(AquaSettledLegsReader::new(
        Arc::new(h.maker_provider.clone()),
        *h.aqua.address(),
    ));
    let recapture = RecaptureService::new(
        oracle,
        store.clone(),
        settled_legs,
        BTreeMap::from([(h.t0, 18u8), (h.t1, 18u8)]),
        policy,
    );
    // Exact-out fill ⇒ the resolver's spread token is the input, t0. The credit is valued from the
    // fill's *actual* legs, read back from its own Aqua events (not the plan's expectation).
    let credits = recapture
        .on_settled(
            intent.id,
            fill_tx.expect("the reverse fill confirmed"),
            &plan.legs,
            plan.expected_profit,
            h.t0,
        )
        .await;
    assert!(
        !credits.is_empty(),
        "the maker earned a recapture credit from the real fill"
    );
    let owed = credits
        .iter()
        .fold(U256::ZERO, |sum, c| sum.saturating_add(c.amount));

    // Payout: rebate the maker on chain from a funded treasury (the taker account).
    MockERC20::new(h.t0, h.taker_provider.clone())
        .mint(h.taker, owed)
        .send()
        .await
        .expect("fund treasury")
        .watch()
        .await
        .expect("fund mined");
    let before = balance_of(h, h.t0, h.maker).await;
    let pre_payout = h.latest_block().await;
    let payer = Arc::new(AlloyRebatePayer::new(h.taker_provider.clone()));
    let payout = PayoutService::new(store.clone(), payer);
    assert_eq!(payout.settle_outstanding().await.expect("settle"), 1);
    let after = balance_of(h, h.t0, h.maker).await;
    let rebate_tx = transfer_tx(h, h.t0, h.maker, pre_payout).await;

    assert_eq!(after, before + owed, "the maker was rebated on chain");
    assert!(
        store.outstanding().await.expect("outstanding").is_empty(),
        "the credit is settled"
    );

    println!("\n========== Solvent recapture — full arc on chain ==========");
    println!("maker:                {}", h.maker);
    println!("pool (t0,t1):         {} / {}", h.t0, h.t1);
    println!(
        "  reserves pre-flow:  ({}, {})  → t1 fair mid {} t0",
        whole(t0_pre),
        whole(t1_pre),
        fair_price_t1.round_dp(4)
    );
    println!(
        "  after forward flow: ({}, {})  → t1 now {} t0 (cheap)",
        whole(t0_post),
        whole(t1_post),
        cheap_price_t1.round_dp(4)
    );
    println!(
        "reverse counter-intent: buy {} t1 for ≤ {} t0 — internalized into the maker",
        whole(output),
        whole(input)
    );
    println!(
        "  filled on chain:    {}",
        fill_tx.map_or("(hash unavailable)".into(), |t| t.to_string())
    );
    println!("  resolver spread:    {} t0", whole(plan.expected_profit));
    println!(
        "recapture credit:     {} t0  (80% of fair-mid LVR, capped by spread)",
        whole(owed)
    );
    println!(
        "maker rebated:        {} → {} t0  (+{})",
        whole(before),
        whole(after),
        whole(owed)
    );
    println!(
        "  rebate transfer:    {}",
        rebate_tx.map_or("(hash unavailable)".into(), |t| t.to_string())
    );
    if let Some(base) = otterscan {
        println!("explorer:");
        if let Some(tx) = fill_tx {
            println!("  reverse fill        {base}/tx/{tx}");
        }
        if let Some(tx) = rebate_tx {
            println!("  maker rebate        {base}/tx/{tx}");
        }
        println!("  maker               {base}/address/{}", h.maker);
    }
    println!("===========================================================\n");
}

#[tokio::test]
async fn e2e_imbalance_then_reverse_fill_recaptures_and_rebates_the_maker() {
    if common::skip_without_anvil() {
        return;
    }
    run_full_arc(&setup().await, None).await;
}

/// The same full arc, run against an already-running node (the docker devnet) so every step —
/// ship, forward swaps, the reverse fill, and the rebate transfer — is browsable in Otterscan.
/// Skipped unless `DEVNET_RPC` is set (so it never fires in the offline gate):
///   `DEVNET_RPC=http://localhost:8545 cargo test -p solvent-adapters --test e2e_recapture_loop \
///      e2e_full_arc_on_devnet -- --ignored --nocapture`
#[tokio::test]
#[ignore = "needs the docker devnet; set DEVNET_RPC"]
async fn e2e_full_arc_on_devnet() {
    let Ok(rpc) = std::env::var("DEVNET_RPC") else {
        eprintln!("DEVNET_RPC unset — skipping the on-devnet full-arc walkthrough");
        return;
    };
    let otterscan =
        std::env::var("OTTERSCAN").unwrap_or_else(|_| "http://localhost:5100".to_string());
    run_full_arc(&setup_attached(&rpc).await, Some(&otterscan)).await;
}
