//! SQLite store integration test — hermetic: an in-memory database, no external
//! service or Docker, so it runs on the default `cargo test`.

use alloy::primitives::{Address, Bytes, B256, U256};
use solvent_adapters::registry::SqliteStore;
use solvent_core::{
    deps::registry::Store,
    primitives::{
        registry::{AquaEvent, EventCursor, EventExt},
        ChainId, MakerId, StrategyHash,
    },
};
use sqlx::sqlite::SqlitePoolOptions;

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

/// A migrated, empty in-memory store. One connection keeps the `:memory:` database
/// alive for the whole test.
async fn setup() -> SqliteStore {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    let store = SqliteStore::new(pool);
    store.migrate().await.expect("migrate");
    store
}

#[tokio::test]
async fn persists_dedupes_and_replays_losslessly() {
    let store = setup().await;

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

    // Replay is in fold order and byte-identical — the JSON round-trip is lossless.
    let replay = store.events(chain).await.unwrap();
    assert_eq!(replay, events);
}
