//! Live E2E — the ledger service against real on-chain budgets. Each test ships strategies, syncs
//! the registry, and wires `LedgerService` over `SqliteLedgerStore` + `AlloyBudgetSource` (wallet
//! budget = `min(balanceOf, allowance)` via Multicall3; strategy virtual = the registry snapshot) +
//! `SystemClock`. Gated on a spawnable anvil.
//!
//! Coverage: two-ceiling admission from live state, the shared-wallet and same-strategy races,
//! the firm-confirm catching a dropped budget, docked/unknown strategies as zero budget, the
//! never-over-promise invariant under real concurrency, mixed-state and idempotent recovery, and the
//! partial-post / void / TTL lifecycle across multiple makers.

mod common;

use std::sync::Arc;

use alloy::{
    primitives::{address, keccak256, Address, Bytes, B256, U256},
    providers::ext::AnvilApi,
};
use common::{pipeline, Harness, StrategySpec, CHAIN};
use solvent_adapters::ledger::{AlloyBudgetSource, SqliteLedgerStore, SystemClock};
use solvent_core::{
    ledger::LedgerService,
    primitives::{
        ledger::{AccountKey, ReservationSource},
        IntentId, MakerId, ReservationId, StrategyHash,
    },
    registry::SharedSnapshot,
};
use sqlx::SqlitePool;
use tempfile::TempDir;

const MULTICALL3: Address = address!("cA11bde05977b3631167028862bE2a173976CA11");
const MULTICALL3_CODE: &str = include_str!("fixtures/multicall3_runtime.hex");

// ---- helpers ----------------------------------------------------------------

fn rid(n: u8) -> ReservationId {
    ReservationId(B256::from([n; 32]))
}
fn iid(n: u8) -> IntentId {
    IntentId(B256::from([n; 32]))
}
fn wallet(maker: MakerId, token: Address) -> AccountKey {
    AccountKey::WalletBudget { maker, token }
}
fn source(
    maker: MakerId,
    strategy: StrategyHash,
    token: Address,
    amount: U256,
) -> ReservationSource {
    ReservationSource {
        maker,
        strategy_hash: strategy,
        token,
        amount,
    }
}

/// A harness with Multicall3 injected — a fresh anvil lacks it, but `AlloyBudgetSource` reads wallet
/// budgets through it.
async fn harness() -> Harness {
    let h = Harness::setup().await;
    let code: Bytes = MULTICALL3_CODE.trim().parse().expect("multicall3 bytecode");
    h.maker_provider
        .anvil_set_code(MULTICALL3, code)
        .await
        .expect("predeploy multicall3");
    h
}

/// The supported, non-guarded strategies from the fixture — the ones the ledger can price and pull.
fn supported(h: &Harness) -> Vec<StrategySpec> {
    h.fx.strategies
        .iter()
        .filter(|s| s.supported && !s.guarded)
        .cloned()
        .collect()
}

/// A fresh, migrated ledger database on its own temp file. The `TempDir` must outlive the pool.
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

/// Sync the registry from `from_block 0` up to the tip and return the populated snapshot (plus the
/// guards that keep its store alive).
async fn synced(h: &Harness) -> (Arc<SharedSnapshot>, SqlitePool, TempDir) {
    let (sync, snapshot, pool, dir) = pipeline(h, CHAIN).await;
    sync.sync_once(h.latest_block().await).await.expect("sync");
    (snapshot, pool, dir)
}

fn service(h: &Harness, snapshot: &Arc<SharedSnapshot>, pool: &SqlitePool) -> LedgerService {
    LedgerService::new(
        Arc::new(SqliteLedgerStore::new(pool.clone())),
        Arc::new(AlloyBudgetSource::new(
            h.maker_provider.clone(),
            *h.aqua.address(),
            h.app,
            snapshot.clone(),
        )),
        Arc::new(SystemClock),
    )
}

fn half(v: U256) -> U256 {
    v / U256::from(2u64)
}
fn quarter(v: U256) -> U256 {
    v / U256::from(4u64)
}
fn one() -> U256 {
    U256::from(1u64)
}

// ---- tests ------------------------------------------------------------------

#[tokio::test]
async fn e2e_reserves_against_real_budgets_and_recovers() {
    if common::skip_without_anvil() {
        return;
    }
    let h = harness().await;
    let spec = supported(&h).remove(0);
    h.ship(&spec).await;
    let (snapshot, _rp, _rd) = synced(&h).await;
    let (pool, _ld) = ledger_pool().await;

    let maker = MakerId(h.maker);
    let sh = StrategyHash(spec.hash());
    let w = wallet(maker, h.t0);
    let v = AccountKey::StrategyVirtual {
        maker,
        strategy_hash: sh,
        token: h.t0,
    };
    let ship = spec.ship_lo;
    let sixty = ship * U256::from(60u64) / U256::from(100u64);

    let svc = service(&h, &snapshot, &pool);
    // Two-ceiling admission from live state: wallet = min(balanceOf, allowance) = ship, virtual = ship.
    svc.reserve(rid(1), iid(1), vec![source(maker, sh, h.t0, sixty)], 60)
        .await
        .expect("reserve");
    assert_eq!(svc.available(&w), ship - sixty);
    assert_eq!(svc.available(&v), ship - sixty);

    // Same-strategy race: a second 60% overdraws the strategy virtual.
    assert!(
        svc.reserve(rid(2), iid(2), vec![source(maker, sh, h.t0, sixty)], 60)
            .await
            .is_err(),
        "a second 60% overdraws the strategy virtual"
    );

    // Crash-recovery: a fresh service over the same store rebuilds the pending hold.
    let restarted = service(&h, &snapshot, &pool);
    assert_eq!(restarted.available(&w), U256::ZERO);
    restarted.recover().await.expect("recover");
    assert_eq!(restarted.available(&w), ship - sixty);

    restarted.post(rid(1), &[sixty]).await.expect("post");
    assert_eq!(restarted.available(&w), ship - sixty);
}

#[tokio::test]
async fn e2e_shared_wallet_binds_across_strategies() {
    if common::skip_without_anvil() {
        return;
    }
    let h = harness().await;
    let specs = supported(&h);
    let s2 = specs.get(1).expect("need two supported strategies").clone();
    let s1 = specs[0].clone();
    h.ship(&s1).await;
    h.ship(&s2).await;
    // Cap the maker's t0 allowance to just s1's amount, so the shared WALLET, not either strategy
    // virtual, is the binding ceiling.
    h.approve_aqua(h.t0, s1.ship_lo).await;

    let (snapshot, _rp, _rd) = synced(&h).await;
    let (pool, _ld) = ledger_pool().await;
    let svc = service(&h, &snapshot, &pool);

    let maker = MakerId(h.maker);
    let (h1, h2) = (StrategyHash(s1.hash()), StrategyHash(s2.hash()));

    // Draw the whole wallet allowance through s1.
    svc.reserve(
        rid(1),
        iid(1),
        vec![source(maker, h1, h.t0, s1.ship_lo)],
        60,
    )
    .await
    .expect("reserve s1");
    assert_eq!(svc.available(&wallet(maker, h.t0)), U256::ZERO);

    // s2 is fully funded on its own virtual, but the shared wallet is exhausted — even 1 wei fails.
    assert!(
        svc.reserve(rid(2), iid(2), vec![source(maker, h2, h.t0, one())], 60)
            .await
            .is_err(),
        "the shared wallet binds across strategies"
    );
}

#[tokio::test]
async fn e2e_firm_confirm_catches_budget_drop() {
    if common::skip_without_anvil() {
        return;
    }
    let h = harness().await;
    let spec = supported(&h).remove(0);
    h.ship(&spec).await;
    let (snapshot, _rp, _rd) = synced(&h).await;
    let (pool, _ld) = ledger_pool().await;
    let svc = service(&h, &snapshot, &pool);

    let maker = MakerId(h.maker);
    let sh = StrategyHash(spec.hash());
    let a = spec.ship_lo;

    svc.reserve(rid(1), iid(1), vec![source(maker, sh, h.t0, half(a))], 60)
        .await
        .expect("reserve half");
    // The maker slashes their Aqua allowance externally.
    h.approve_aqua(h.t0, quarter(a)).await;
    // The next reserve's JIT confirm reads the reduced budget and rejects.
    assert!(
        svc.reserve(
            rid(2),
            iid(2),
            vec![source(maker, sh, h.t0, quarter(a))],
            60
        )
        .await
        .is_err(),
        "the firm confirm catches a dropped allowance"
    );
}

#[tokio::test]
async fn e2e_inactive_or_unknown_strategy_has_zero_budget() {
    if common::skip_without_anvil() {
        return;
    }
    let h = harness().await;
    let spec = supported(&h).remove(0);
    h.ship(&spec).await;
    let (sync, snapshot, _rp, _rd) = pipeline(&h, CHAIN).await;
    sync.sync_once(h.latest_block().await).await.expect("sync");
    let (pool, _ld) = ledger_pool().await;
    let svc = service(&h, &snapshot, &pool);
    let maker = MakerId(h.maker);

    // Unknown strategy: absent from the snapshot → zero virtual budget → rejected.
    let unknown = StrategyHash(B256::from([0xEE; 32]));
    assert!(
        svc.reserve(
            rid(1),
            iid(1),
            vec![source(maker, unknown, h.t0, one())],
            60
        )
        .await
        .is_err(),
        "an unknown strategy has no budget"
    );

    // Docked strategy: kept as a tombstone with its old balance, but no longer pullable → zero budget.
    h.dock(&spec).await;
    sync.sync_once(h.latest_block().await)
        .await
        .expect("re-sync after dock");
    let sh = StrategyHash(spec.hash());
    assert!(
        svc.reserve(rid(2), iid(2), vec![source(maker, sh, h.t0, one())], 60)
            .await
            .is_err(),
        "a docked strategy has no budget"
    );
}

#[tokio::test]
async fn e2e_concurrent_reserves_never_over_promise() {
    if common::skip_without_anvil() {
        return;
    }
    let h = harness().await;
    let spec = supported(&h).remove(0);
    h.ship(&spec).await;
    let (snapshot, _rp, _rd) = synced(&h).await;
    let (pool, _ld) = ledger_pool().await;
    let svc = Arc::new(service(&h, &snapshot, &pool));

    let maker = MakerId(h.maker);
    let sh = StrategyHash(spec.hash());
    let a = spec.ship_lo;
    let fifth = a / U256::from(5u64);
    let t0 = h.t0;

    // Fire eight 20% reserves at once against a budget that fits exactly five.
    let attempts = (0..8u8).map(|i| {
        let svc = svc.clone();
        async move {
            svc.reserve(
                rid(10 + i),
                iid(10 + i),
                vec![source(maker, sh, t0, fifth)],
                60,
            )
            .await
        }
    });
    let results = futures::future::join_all(attempts).await;
    let admitted = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(
        admitted, 5,
        "exactly the feasible five admit — never over-promise"
    );
    assert_eq!(
        svc.available(&wallet(maker, t0)),
        a - fifth * U256::from(5u64)
    );
}

#[tokio::test]
async fn e2e_recovery_of_mixed_states_is_idempotent() {
    if common::skip_without_anvil() {
        return;
    }
    let h = harness().await;
    let spec = supported(&h).remove(0);
    h.ship(&spec).await;
    let (snapshot, _rp, _rd) = synced(&h).await;
    let (pool, _ld) = ledger_pool().await;

    let maker = MakerId(h.maker);
    let sh = StrategyHash(spec.hash());
    let a = spec.ship_lo;
    let q = quarter(a);
    let w = wallet(maker, h.t0);

    let svc = service(&h, &snapshot, &pool);
    for n in 1..=3u8 {
        svc.reserve(rid(n), iid(n), vec![source(maker, sh, h.t0, q)], 60)
            .await
            .expect("reserve");
    }
    svc.post(rid(1), &[q]).await.expect("post r1");
    svc.void(rid(2)).await.expect("void r2");
    // Live: r3 pending + r1 consumed → available = a − 2q.
    assert_eq!(svc.available(&w), a - q - q);

    // Restart: a fresh service rebuilds only the pending hold (r3). A posted reservation's consumed
    // is not rebuilt — in production the chain balance already reflects the pull.
    let restarted = service(&h, &snapshot, &pool);
    restarted.recover().await.expect("recover");
    assert_eq!(
        restarted.available(&w),
        a - q,
        "only the pending hold is rebuilt"
    );

    // Recovery is idempotent — replaying it does not double-count.
    restarted.recover().await.expect("recover again");
    assert_eq!(restarted.available(&w), a - q, "recover is idempotent");
}

#[tokio::test]
async fn e2e_lifecycle_partial_post_void_and_ttl() {
    if common::skip_without_anvil() {
        return;
    }
    let h = harness().await;
    let spec = supported(&h).remove(0);
    h.ship(&spec).await;
    let (snapshot, _rp, _rd) = synced(&h).await;
    let (pool, _ld) = ledger_pool().await;
    let svc = service(&h, &snapshot, &pool);

    let maker = MakerId(h.maker);
    let sh = StrategyHash(spec.hash());
    let a = spec.ship_lo;
    let q = quarter(a);
    let w = wallet(maker, h.t0);

    // Partial post: reserve half, pull only a quarter — the unfilled quarter returns to available.
    svc.reserve(rid(1), iid(1), vec![source(maker, sh, h.t0, half(a))], 60)
        .await
        .expect("reserve r1");
    svc.post(rid(1), &[q]).await.expect("partial post");
    assert_eq!(
        svc.available(&w),
        a - q,
        "partial post restores the remainder"
    );

    // Void frees the hold, making the capacity reusable.
    svc.reserve(rid(2), iid(2), vec![source(maker, sh, h.t0, q)], 60)
        .await
        .expect("reserve r2");
    assert_eq!(svc.available(&w), a - q - q);
    svc.void(rid(2)).await.expect("void r2");
    assert_eq!(svc.available(&w), a - q, "void frees the hold");

    // TTL: a zero-lived reservation is swept immediately, restoring its hold.
    svc.reserve(rid(3), iid(3), vec![source(maker, sh, h.t0, q)], 0)
        .await
        .expect("reserve r3 ttl=0");
    assert_eq!(svc.available(&w), a - q - q);
    assert_eq!(svc.sweep_expired().await.expect("sweep"), 1);
    assert_eq!(svc.available(&w), a - q, "the expired hold is restored");
}

#[tokio::test]
async fn e2e_multi_maker_budgets_are_independent() {
    if common::skip_without_anvil() {
        return;
    }
    let h = harness().await;
    let spec = supported(&h).remove(0);
    h.ship(&spec).await; // maker 1
    h.ship_second_maker().await; // maker 2 (the taker account)
    let (snapshot, _rp, _rd) = synced(&h).await;
    let (pool, _ld) = ledger_pool().await;
    let svc = service(&h, &snapshot, &pool);

    let m1 = MakerId(h.maker);
    let sh1 = StrategyHash(spec.hash());
    let m2 = MakerId(h.taker);
    let sm = &h.fx.second_maker;
    let sh2 = StrategyHash(keccak256(&sm.strategy_hex));

    // Exhaust maker 1's t0 wallet.
    svc.reserve(
        rid(1),
        iid(1),
        vec![source(m1, sh1, h.t0, spec.ship_lo)],
        60,
    )
    .await
    .expect("reserve m1");
    assert_eq!(svc.available(&wallet(m1, h.t0)), U256::ZERO);

    // Maker 2 reserves against its own, untouched budget.
    svc.reserve(rid(2), iid(2), vec![source(m2, sh2, h.t0, sm.ship_lo)], 60)
        .await
        .expect("reserve m2 independent of m1");
    assert_eq!(svc.available(&wallet(m2, h.t0)), U256::ZERO);
    assert_eq!(
        svc.available(&wallet(m1, h.t0)),
        U256::ZERO,
        "m1 unaffected"
    );
}
