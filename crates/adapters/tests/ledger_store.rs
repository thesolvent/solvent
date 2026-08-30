//! SQLite ledger store integration test — hermetic: an in-memory database, no external service or
//! Docker, so it runs on the default `cargo test`. Covers the durable two-phase lifecycle,
//! idempotent reserve, state-guarded transitions, and crash-recovery of the still-open reservations.

use alloy::primitives::{Address, B256, U256};
use solvent_adapters::ledger::SqliteLedgerStore;
use solvent_core::{
    deps::ledger::LedgerStore,
    primitives::{
        ledger::{Reservation, ReservationSource},
        IntentId, MakerId, ReservationId, StrategyHash,
    },
};
use sqlx::sqlite::SqlitePoolOptions;

fn source(m: u8, s: u8, t: u8, amount: u64) -> ReservationSource {
    ReservationSource {
        maker: MakerId(Address::from([m; 20])),
        strategy_hash: StrategyHash(B256::from([s; 32])),
        token: Address::from([t; 20]),
        amount: U256::from(amount),
    }
}

fn reservation(id: u8, sources: Vec<ReservationSource>) -> Reservation {
    Reservation::new(
        ReservationId(B256::from([id; 32])),
        IntentId(B256::from([id; 32])),
        sources,
        1_700_000_000 + id as u64,
    )
}

fn rid(id: u8) -> ReservationId {
    ReservationId(B256::from([id; 32]))
}

/// A migrated, empty in-memory store. One connection keeps the `:memory:` database alive.
async fn setup() -> SqliteLedgerStore {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    let store = SqliteLedgerStore::new(pool);
    store.migrate().await.expect("migrate");
    store
}

#[tokio::test]
async fn recovers_only_the_open_reservations() {
    let store = setup().await;
    let a = reservation(1, vec![source(1, 1, 3, 600_000), source(1, 2, 3, 100_000)]);
    let b = reservation(2, vec![source(2, 1, 3, 200_000)]);
    let c = reservation(3, vec![source(3, 1, 3, 300_000)]);
    store.reserve(&a).await.unwrap();
    store.reserve(&b).await.unwrap();
    store.reserve(&c).await.unwrap();

    // A settles, B is voided — both leave the open set.
    store
        .post(rid(1), &[U256::from(600_000u64), U256::from(100_000u64)])
        .await
        .unwrap();
    store.void(rid(2)).await.unwrap();

    // Recovery replays only C, with its multi-source body round-tripped losslessly.
    let open = store.open_reservations().await.unwrap();
    assert_eq!(open, vec![c]);
}

#[tokio::test]
async fn reserve_is_idempotent() {
    let store = setup().await;
    let a = reservation(1, vec![source(1, 1, 3, 500_000)]);
    store.reserve(&a).await.unwrap();
    // A duplicate id (even with a different body) neither overwrites nor duplicates.
    store
        .reserve(&reservation(1, vec![source(9, 9, 9, 1)]))
        .await
        .unwrap();
    assert_eq!(store.open_reservations().await.unwrap(), vec![a]);
}

#[tokio::test]
async fn transitions_only_fire_from_the_expected_state() {
    let store = setup().await;
    store
        .reserve(&reservation(1, vec![source(1, 1, 3, 500_000)]))
        .await
        .unwrap();

    store.post(rid(1), &[U256::from(500_000u64)]).await.unwrap();
    // Re-posting, or voiding an already-posted reservation, is a guarded no-op.
    store.post(rid(1), &[U256::from(1u64)]).await.unwrap();
    store.void(rid(1)).await.unwrap();
    assert!(store.open_reservations().await.unwrap().is_empty());

    // void_reorg re-opens a posted reservation to reorg_open — still off the pending set.
    store.void_reorg(rid(1)).await.unwrap();
    assert!(store.open_reservations().await.unwrap().is_empty());
}
