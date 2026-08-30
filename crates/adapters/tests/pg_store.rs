//! Postgres store integration test — env-gated on `TEST_DATABASE_URL` so the
//! default `cargo test` stays hermetic. Run against an ephemeral database with
//! `scripts/with-postgres.sh cargo test -p solvent-adapters --test pg_store`.

use alloy::primitives::{Address, Bytes, B256, U256};
use solvent_adapters::registry::PgStore;
use solvent_core::{
    deps::registry::Store,
    primitives::{
        registry::{AquaEvent, EventCursor, EventExt},
        ChainId, MakerId, StrategyHash,
    },
};
use sqlx::PgPool;

fn shipped(s: u8) -> AquaEvent {
    AquaEvent::Shipped {
        maker: MakerId(Address::from([s; 20])),
        app: Address::from([0xAA; 20]),
        strategy_hash: StrategyHash(B256::from([s; 32])),
        strategy: Bytes::from(vec![s]),
    }
}

fn pushed(s: u8, token: u8, amount: u64) -> AquaEvent {
    AquaEvent::Pushed {
        maker: MakerId(Address::from([s; 20])),
        app: Address::from([0xAA; 20]),
        strategy_hash: StrategyHash(B256::from([s; 32])),
        token: Address::from([token; 20]),
        amount: U256::from(amount),
    }
}

fn ext(block: u64, log: u64, event: AquaEvent) -> EventExt<AquaEvent> {
    EventExt {
        event,
        address: Address::from([0xAA; 20]),
        block_hash: Some(B256::from([block as u8; 32])),
        block_number: Some(block),
        transaction_hash: Some(B256::from([0x11; 32])),
        transaction_index: Some(3),
        log_index: Some(log),
        removed: false,
    }
}

/// Connect, migrate, and truncate — or `None` when `TEST_DATABASE_URL` is unset.
async fn setup() -> Option<PgStore> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let pool = PgPool::connect(&url)
        .await
        .expect("connect to TEST_DATABASE_URL");
    let store = PgStore::new(pool.clone());
    store.migrate().await.expect("migrate");
    sqlx::query("TRUNCATE aqua_event, registry_cursor")
        .execute(&pool)
        .await
        .expect("truncate");
    Some(store)
}

#[tokio::test]
async fn persists_dedupes_and_replays_losslessly() {
    let Some(store) = setup().await else {
        eprintln!("TEST_DATABASE_URL unset — skipping Postgres integration test");
        return;
    };

    let chain = ChainId(1);
    let events = vec![ext(10, 0, shipped(1)), ext(10, 1, pushed(1, 2, 1000))];
    let cursor = EventCursor {
        block_number: 10,
        log_index: 1,
    };

    // Both events are new.
    let inserted = store.insert(chain, &events).await.unwrap();
    assert_eq!(inserted.len(), 2);

    // Re-inserting the same range (an overlap re-scan) adds nothing.
    let again = store.insert(chain, &events).await.unwrap();
    assert!(again.is_empty(), "overlap re-scan must not re-insert");

    // The cursor round-trips.
    store.save_cursor(chain, cursor).await.unwrap();
    assert_eq!(store.cursor(chain).await.unwrap(), Some(cursor));

    // Replay is in fold order and byte-identical — the JSONB round-trip is lossless.
    let replay = store.events(chain).await.unwrap();
    assert_eq!(replay, events);
}
