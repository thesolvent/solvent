//! SQLite recapture store integration test — hermetic: an in-memory database, no external service,
//! so it runs on the default `cargo test`. Covers intent-keyed accrual idempotency, the outstanding
//! read, and settling a credit off the outstanding set.

use alloy::primitives::{Address, B256, U256};
use solvent_adapters::recapture::SqliteRecaptureStore;
use solvent_core::{
    deps::recapture::RecaptureStore,
    primitives::{
        recapture::{AccruedCredit, RecaptureCredit},
        IntentId, MakerId,
    },
};
use sqlx::sqlite::SqlitePoolOptions;

fn intent(n: u8) -> IntentId {
    IntentId(B256::from([n; 32]))
}
fn credit(maker: u8, token: u8, amount: u64) -> RecaptureCredit {
    RecaptureCredit::new(
        MakerId(Address::from([maker; 20])),
        Address::from([token; 20]),
        U256::from(amount),
    )
}

/// A migrated, empty in-memory store. One connection keeps the `:memory:` database alive.
async fn setup() -> SqliteRecaptureStore {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    let store = SqliteRecaptureStore::new(pool);
    store.migrate().await.expect("migrate");
    store
}

#[tokio::test]
async fn accrue_is_idempotent_per_intent_maker_token() {
    let store = setup().await;
    let c = credit(5, 2, 240);
    store
        .accrue(intent(1), std::slice::from_ref(&c))
        .await
        .unwrap();
    // A re-driven reconcile re-accrues the same fill (even with a different amount) — kept once.
    store.accrue(intent(1), &[credit(5, 2, 999)]).await.unwrap();
    assert_eq!(
        store.outstanding().await.unwrap(),
        vec![AccruedCredit::new(intent(1), c)]
    );
}

#[tokio::test]
async fn mark_settled_removes_from_outstanding() {
    let store = setup().await;
    let a = AccruedCredit::new(intent(1), credit(5, 2, 240));
    let b = AccruedCredit::new(intent(2), credit(6, 2, 100));
    store
        .accrue(a.intent, std::slice::from_ref(&a.credit))
        .await
        .unwrap();
    store
        .accrue(b.intent, std::slice::from_ref(&b.credit))
        .await
        .unwrap();
    assert_eq!(store.outstanding().await.unwrap().len(), 2);

    store.mark_settled(std::slice::from_ref(&a)).await.unwrap();
    assert_eq!(store.outstanding().await.unwrap(), vec![b]);
    // Settling again is a harmless no-op.
    store.mark_settled(std::slice::from_ref(&a)).await.unwrap();
    assert_eq!(store.outstanding().await.unwrap().len(), 1);
}
