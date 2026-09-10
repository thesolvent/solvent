//! Live E2E — the three-component decision path end to end: sync the registry from a real anvil,
//! `route` an intent against the ledger's caps, and `reserve` the resulting plan. Asserts the
//! cross-component invariant that a plan the router produces (respecting the frozen caps) is always
//! reservable by the ledger, and that the router declines what exceeds the caps. Gated on anvil.

mod common;

use std::collections::{btree_map::Entry, BTreeMap};
use std::sync::Arc;

use alloy::{
    primitives::{address, Address, Bytes, B256, U256},
    providers::ext::AnvilApi,
};
use common::{pipeline, Harness, StrategySpec, CHAIN};
use solvent_adapters::ledger::{AlloyBudgetSource, SqliteLedgerStore, SystemClock};
use solvent_core::{
    deps::ledger::BudgetSource,
    ledger::{AvailableSnapshot, LedgerService},
    primitives::{
        ledger::{AccountKey, ReservationSource},
        registry::{Snapshot, TokenPair},
        routing::{RouteRequest, RoutingConfig},
        IntentId, ReservationId,
    },
    registry::SharedSnapshot,
    routing::{route, GuardSnapshot, RoutingBook},
};
use sqlx::SqlitePool;
use tempfile::TempDir;

const MULTICALL3: Address = address!("cA11bde05977b3631167028862bE2a173976CA11");
const MULTICALL3_CODE: &str = include_str!("fixtures/multicall3_runtime.hex");

fn iid(n: u8) -> IntentId {
    IntentId(B256::from([n; 32]))
}
fn rid(n: u8) -> ReservationId {
    ReservationId(B256::from([n; 32]))
}

/// A harness with Multicall3 injected (the budget source reads wallet budgets through it).
async fn harness() -> Harness {
    let h = Harness::setup().await;
    let code: Bytes = MULTICALL3_CODE.trim().parse().expect("multicall3 bytecode");
    h.maker_provider
        .anvil_set_code(MULTICALL3, code)
        .await
        .expect("predeploy multicall3");
    h
}

fn supported(h: &Harness) -> Vec<StrategySpec> {
    h.fx.strategies
        .iter()
        .filter(|s| s.supported && !s.guarded)
        .cloned()
        .collect()
}

async fn synced(h: &Harness) -> (Arc<SharedSnapshot>, SqlitePool, TempDir) {
    let (sync, snapshot, pool, dir) = pipeline(h, CHAIN).await;
    sync.sync_once(h.latest_block().await).await.expect("sync");
    (snapshot, pool, dir)
}

async fn ledger_pool() -> (SqlitePool, TempDir) {
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
    (pool, dir)
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

fn service(h: &Harness, snapshot: &Arc<SharedSnapshot>, pool: &SqlitePool) -> LedgerService {
    LedgerService::new(
        Arc::new(SqliteLedgerStore::new(pool.clone())),
        Arc::new(budget_source(h, snapshot)),
        Arc::new(SystemClock),
    )
}

/// The quote-time caps: for every strategy on the pair, the wallet + strategy-virtual budgets on the
/// payout token, read from the budget source. This is what the router freezes `cap_out` against.
async fn quote_caps(
    snap: &Snapshot,
    budgets: &AlloyBudgetSource<alloy::providers::DynProvider>,
    token_in: Address,
    token_out: Address,
) -> AvailableSnapshot {
    let pair = TokenPair::new(token_in, token_out);
    let mut map: BTreeMap<AccountKey, U256> = BTreeMap::new();
    for s in snap.active_strategies_for_pair(pair) {
        let accounts = [
            AccountKey::WalletBudget {
                maker: s.key.maker,
                token: token_out,
            },
            AccountKey::StrategyVirtual {
                maker: s.key.maker,
                strategy_hash: s.key.strategy_hash,
                token: token_out,
            },
        ];
        for account in accounts {
            if let Entry::Vacant(e) = map.entry(account) {
                e.insert(budgets.budget(&account).await.unwrap_or(U256::ZERO));
            }
        }
    }
    AvailableSnapshot(map)
}

/// Convert a routed plan's legs into the reservation sources the ledger holds — each leg reserves
/// its output on the payout token from its maker's strategy.
fn sources_of(plan: &solvent_core::primitives::routing::RoutePlan) -> Vec<ReservationSource> {
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

/// Total payout-token capacity across the caps — the sum of the per-strategy virtual budgets.
fn total_virtual(caps: &AvailableSnapshot, token_out: Address) -> U256 {
    caps.0
        .iter()
        .filter(
            |(k, _)| matches!(k, AccountKey::StrategyVirtual { token, .. } if *token == token_out),
        )
        .fold(U256::ZERO, |s, (_, v)| s + *v)
}

#[tokio::test]
async fn e2e_routed_plan_is_always_reservable() {
    if common::skip_without_anvil() {
        return;
    }
    let h = harness().await;
    for spec in supported(&h).iter().take(3) {
        h.ship(spec).await;
    }
    let (snapshot, _rp, _rd) = synced(&h).await;
    let (pool, _ld) = ledger_pool().await;
    let svc = service(&h, &snapshot, &pool);
    let budgets = budget_source(&h, &snapshot);

    let snap = snapshot.load();
    // Sell t0 for t1; caps are on the payout token t1.
    let caps = quote_caps(&snap, &budgets, h.t0, h.t1).await;
    let total = total_virtual(&caps, h.t1);
    assert!(total > U256::ZERO, "strategies fund the payout token");

    let cfg = RoutingConfig::new(64, 8, 150_000);
    // A comfortably interior exact-out trade: a quarter of the book's payout capacity.
    let target = total / U256::from(4u64);
    let req = RouteRequest {
        intent: iid(1),
        token_in: h.t0,
        token_out: h.t1,
        amount: target,
        exact_in: false,
    };
    // A generous max-in bound: this test checks reservability, not the profit gate.
    let plan = route(
        RoutingBook::new(&snap, &caps, &GuardSnapshot::default()),
        &req,
        U256::MAX,
        &cfg,
        U256::ZERO,
        None,
    )
    .expect("router produces a plan for an interior trade");
    assert!(!plan.legs.is_empty());

    let sources = sources_of(&plan);
    svc.reserve(rid(1), plan.intent, sources.clone(), 60)
        .await
        .expect("a plan the router froze against these caps must reserve");

    // The hold shows up: each leg's payout account has less room than before.
    for src in &sources {
        let v = AccountKey::StrategyVirtual {
            maker: src.maker,
            strategy_hash: src.strategy_hash,
            token: src.token,
        };
        assert!(
            svc.available(&v) < caps.available(&v),
            "reserving the plan reduced the strategy-virtual room"
        );
    }
}

#[tokio::test]
async fn e2e_router_declines_beyond_caps() {
    if common::skip_without_anvil() {
        return;
    }
    let h = harness().await;
    for spec in supported(&h).iter().take(3) {
        h.ship(spec).await;
    }
    let (snapshot, _rp, _rd) = synced(&h).await;
    let budgets = budget_source(&h, &snapshot);
    let snap = snapshot.load();
    let caps = quote_caps(&snap, &budgets, h.t0, h.t1).await;
    let total = total_virtual(&caps, h.t1);

    let cfg = RoutingConfig::new(64, 8, 150_000);
    // Demand double the whole book's payout capacity — no split can fill it.
    let req = RouteRequest {
        intent: iid(2),
        token_in: h.t0,
        token_out: h.t1,
        amount: total * U256::from(2u64),
        exact_in: false,
    };
    assert!(
        route(
            RoutingBook::new(&snap, &caps, &GuardSnapshot::default()),
            &req,
            U256::MAX,
            &cfg,
            U256::ZERO,
            None
        )
        .is_none(),
        "the router declines a trade beyond the book's capped capacity"
    );
}
