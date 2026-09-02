//! Live full-loop E2E: a self-hosted UniswapX order → normalize → route → reserve → **fill on-chain**
//! through the deployed reactor + `UniswapXAquaFiller`, sourcing the output from a shipped Aqua maker.
//! The proof that the whole ingest→fill path holds against the real contracts. Gated on anvil.

mod common;

use std::sync::Arc;

use alloy::primitives::{address, Address, Bytes, U256};
use alloy::providers::ext::AnvilApi;
use alloy::providers::Provider;
use alloy::rpc::types::{TransactionInput, TransactionRequest};
use alloy::sol;

use common::{pipeline, Harness, MockERC20, StrategySpec, CHAIN};
use solvent_adapters::ingest::uniswapx::{
    OrderSpec, SelfHostedFeed, SignedOrderBuilder, UniswapXFillBuilder, UniswapXV2Normalizer,
};
use solvent_adapters::ledger::{AlloyBudgetSource, SqliteLedgerStore, SystemClock};
use solvent_core::deps::ingest::{FillBuilder, Normalizer, OrderFeed};
use solvent_core::deps::ledger::BudgetSource;
use solvent_core::ledger::{AvailableSnapshot, LedgerService};
use solvent_core::primitives::ingest::RawOrder;
use solvent_core::primitives::ledger::{AccountKey, ReservationSource};
use solvent_core::primitives::registry::{Snapshot, TokenPair};
use solvent_core::primitives::routing::{RouteRequest, RoutingConfig};
use solvent_core::primitives::ReservationId;
use solvent_core::registry::SharedSnapshot;
use solvent_core::routing::route;

use alloy::signers::local::PrivateKeySigner;
use futures::StreamExt;
use sqlx::SqlitePool;
use tempfile::TempDir;

sol!(
    #[sol(rpc)]
    V2DutchOrderReactor,
    "tests/fixtures/artifacts/V2DutchOrderReactor.json"
);
sol!(
    #[sol(rpc)]
    UniswapXAquaFiller,
    "tests/fixtures/artifacts/UniswapXAquaFiller.json"
);

const MULTICALL3: Address = address!("cA11bde05977b3631167028862bE2a173976CA11");
const MULTICALL3_CODE: &str = include_str!("fixtures/multicall3_runtime.hex");
const PERMIT2: Address = address!("000000000022D473030F116dDEE9F6B43aC78BA3");
const PERMIT2_CODE: &str = include_str!("fixtures/permit2_runtime.hex");

fn rid(n: u8) -> ReservationId {
    ReservationId(alloy::primitives::B256::from([n; 32]))
}

/// The deployed stack plus the reactor + filler for the fill path.
struct Stack {
    h: Harness,
    reactor: Address,
    filler: Address,
    chain_id: u64,
}

async fn setup() -> Stack {
    let h = Harness::setup().await;
    let etch = |addr: Address, code: &str| {
        let bytes: Bytes = code.trim().parse().expect("bytecode");
        (addr, bytes)
    };
    for (addr, code) in [
        etch(MULTICALL3, MULTICALL3_CODE),
        etch(PERMIT2, PERMIT2_CODE),
    ] {
        h.maker_provider
            .anvil_set_code(addr, code)
            .await
            .expect("etch");
    }
    let reactor = V2DutchOrderReactor::deploy(h.maker_provider.clone(), PERMIT2, Address::ZERO)
        .await
        .expect("deploy reactor");
    let filler = UniswapXAquaFiller::deploy(h.maker_provider.clone(), h.maker)
        .await
        .expect("deploy filler");
    let chain_id = h.maker_provider.get_chain_id().await.expect("chain id");
    Stack {
        h,
        reactor: *reactor.address(),
        filler: *filler.address(),
        chain_id,
    }
}

async fn synced(h: &Harness) -> (Arc<SharedSnapshot>, SqlitePool, TempDir) {
    let (sync, snapshot, pool, dir) = pipeline(h, CHAIN).await;
    sync.sync_once(h.latest_block().await).await.expect("sync");
    (snapshot, pool, dir)
}

fn budget_source(
    h: &Harness,
    snapshot: &Arc<SharedSnapshot>,
) -> AlloyBudgetSource<alloy::providers::DynProvider> {
    AlloyBudgetSource::new(
        h.maker_provider.clone(),
        *h.aqua.address(),
        h.app,
        snapshot.clone(),
    )
}

async fn ledger(h: &Harness, snapshot: &Arc<SharedSnapshot>) -> (LedgerService, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!(
        "sqlite://{}?mode=rwc",
        dir.path().join("ledger.db").display()
    );
    let pool = SqlitePool::connect(&url).await.expect("open ledger sqlite");
    SqliteLedgerStore::new(pool.clone())
        .migrate()
        .await
        .expect("migrate ledger");
    let svc = LedgerService::new(
        Arc::new(SqliteLedgerStore::new(pool)),
        Arc::new(budget_source(h, snapshot)),
        Arc::new(SystemClock),
    );
    (svc, dir)
}

/// Quote-time caps for the pair, on the payout token, read from the budget source.
async fn quote_caps(
    snap: &Snapshot,
    budgets: &AlloyBudgetSource<alloy::providers::DynProvider>,
    token_out: Address,
    token_in: Address,
) -> AvailableSnapshot {
    let mut map = std::collections::BTreeMap::new();
    for s in snap.active_strategies_for_pair(TokenPair::new(token_in, token_out)) {
        for account in [
            AccountKey::WalletBudget {
                maker: s.key.maker,
                token: token_out,
            },
            AccountKey::StrategyVirtual {
                maker: s.key.maker,
                strategy_hash: s.key.strategy_hash,
                token: token_out,
            },
        ] {
            if let std::collections::btree_map::Entry::Vacant(e) = map.entry(account) {
                e.insert(budgets.budget(&account).await.unwrap_or(U256::ZERO));
            }
        }
    }
    AvailableSnapshot(map)
}

fn first_supported(h: &Harness) -> StrategySpec {
    h.fx.strategies
        .iter()
        .find(|s| s.supported && !s.guarded)
        .cloned()
        .expect("a supported strategy")
}

async fn balance_of(h: &Harness, token: Address, who: Address) -> U256 {
    MockERC20::new(token, h.maker_provider.clone())
        .balanceOf(who)
        .call()
        .await
        .expect("balanceOf")
}

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
    let calldata = UniswapXFillBuilder::new(h.app)
        .build(&intent, &plan, &snap)
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
