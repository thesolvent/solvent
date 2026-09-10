//! SQLite ledger store integration test — hermetic: an in-memory database, no external service or
//! Docker, so it runs on the default `cargo test`. Covers the durable two-phase lifecycle,
//! idempotent reserve, state-guarded transitions, and crash-recovery of the still-open reservations.

use alloy::primitives::{Address, B256, U256};
use solvent_adapters::ledger::SqliteLedgerStore;
use solvent_core::{
    deps::ledger::{LedgerStore, LedgerStoreError},
    primitives::{
        ledger::{Reservation, ReservationSource},
        IntentId, MakerId, RebateBatchId, ReservationId, StrategyHash,
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
    Reservation::for_swap(
        ReservationId(B256::from([id; 32])),
        IntentId(B256::from([id; 32])),
        sources,
        1_700_000_000 + id as u64,
    )
}

fn rebate_reservation(id: u8, sources: Vec<ReservationSource>) -> Reservation {
    Reservation::for_rebate(
        ReservationId(B256::from([id; 32])),
        RebateBatchId(B256::from([id; 32])),
        sources,
        1_700_000_000 + id as u64,
    )
}

fn rid(id: u8) -> ReservationId {
    ReservationId(B256::from([id; 32]))
}

#[derive(sqlx::FromRow)]
struct MigratedReservationRow {
    id: Vec<u8>,
    owner_kind: String,
    owner_id: Vec<u8>,
    state: String,
    filled: Option<String>,
    expires_at: i64,
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
async fn owner_migration_preserves_populated_legacy_rows() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    sqlx::raw_sql(include_str!("../migrations/0002_ledger.sql"))
        .execute(&pool)
        .await
        .expect("create legacy ledger schema");

    let pending = reservation(1, vec![source(1, 1, 3, 500_000)]);
    let posted = reservation(2, vec![source(2, 2, 4, 600_000)]);
    for (reservation, state, filled) in [
        (&pending, "pending", None),
        (&posted, "posted", Some("[\"0x927c0\"]")),
    ] {
        sqlx::query(
            "INSERT INTO ledger_reservation (id, intent, sources, state, filled, expires_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(reservation.id.0.to_vec())
        .bind(reservation.owner.id().to_vec())
        .bind(serde_json::to_string(&reservation.sources).expect("serialize sources"))
        .bind(state)
        .bind(filled)
        .bind(i64::try_from(reservation.expires_at).expect("fixture expiry fits i64"))
        .execute(&pool)
        .await
        .expect("insert legacy reservation");
    }

    sqlx::raw_sql(include_str!("../migrations/0010_ledger_owner.sql"))
        .execute(&pool)
        .await
        .expect("migrate owner schema");

    let rows: Vec<MigratedReservationRow> = sqlx::query_as(
        "SELECT id, owner_kind, owner_id, state, filled, expires_at
         FROM ledger_reservation ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("read migrated rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, pending.id.0.to_vec());
    assert_eq!(rows[0].owner_kind, "swap");
    assert_eq!(rows[0].owner_id, pending.owner.id().to_vec());
    assert_eq!(rows[0].state, "pending");
    assert_eq!(rows[0].filled, None);
    assert_eq!(rows[0].expires_at, pending.expires_at as i64);
    assert_eq!(rows[1].id, posted.id.0.to_vec());
    assert_eq!(rows[1].owner_kind, "swap");
    assert_eq!(rows[1].owner_id, posted.owner.id().to_vec());
    assert_eq!(rows[1].state, "posted");
    assert_eq!(rows[1].filled.as_deref(), Some("[\"0x927c0\"]"));
    assert_eq!(rows[1].expires_at, posted.expires_at as i64);

    let store = SqliteLedgerStore::new(pool);
    assert_eq!(store.open_reservations().await.unwrap(), vec![pending]);
}

#[tokio::test]
async fn recovers_only_the_open_reservations() {
    let store = setup().await;
    let a = reservation(1, vec![source(1, 1, 3, 600_000), source(1, 2, 3, 100_000)]);
    let b = reservation(2, vec![source(2, 1, 3, 200_000)]);
    let c = rebate_reservation(3, vec![source(3, 1, 3, 300_000)]);
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
async fn reserve_accepts_only_an_exact_pending_duplicate() {
    let store = setup().await;
    let a = reservation(1, vec![source(1, 1, 3, 500_000)]);
    store.reserve(&a).await.unwrap();
    store.reserve(&a).await.unwrap();

    let mut different_expiry = a.clone();
    different_expiry.expires_at += 1;
    for conflicting in [
        rebate_reservation(1, a.sources.clone()),
        reservation(1, vec![source(9, 9, 9, 1)]),
        different_expiry,
    ] {
        let conflict = store.reserve(&conflicting).await.unwrap_err();
        assert!(matches!(conflict, LedgerStoreError::Conflict(id) if id == rid(1)));
    }
    assert_eq!(store.open_reservations().await.unwrap(), vec![a]);

    store.void(rid(1)).await.unwrap();
    let terminal = store
        .reserve(&reservation(1, vec![source(1, 1, 3, 500_000)]))
        .await
        .unwrap_err();
    assert!(matches!(terminal, LedgerStoreError::Conflict(id) if id == rid(1)));
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
