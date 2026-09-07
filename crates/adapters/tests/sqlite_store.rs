//! SQLite store integration test — hermetic: an in-memory database, no external
//! service or Docker, so it runs on the default `cargo test`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use alloy::primitives::{Address, Bytes, B256, U256};
use solvent_adapters::registry::SqliteStore;
use solvent_core::{
    deps::ledger::Clock,
    deps::registry::EventStore,
    primitives::{
        registry::{AquaEvent, EventCursor, EventExt},
        ChainId, MakerId, StrategyHash,
    },
};
use sqlx::sqlite::SqlitePoolOptions;

struct ManualClock(AtomicU64);
impl Clock for ManualClock {
    fn now_unix(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

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

#[tokio::test]
async fn recent_pages_newest_first_and_count_since_windows() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    let clock = Arc::new(ManualClock(AtomicU64::new(1000)));
    let store = SqliteStore::with_clock(pool, clock.clone());
    store.migrate().await.expect("migrate");
    let chain = ChainId(1);

    // Three events at increasing blocks, each recorded at a distinct time.
    store
        .insert(chain, &[ext(10, 0, shipped(1))])
        .await
        .unwrap();
    clock.0.store(2000, Ordering::SeqCst);
    store
        .insert(chain, &[ext(11, 0, pushed(1, 2, 5))])
        .await
        .unwrap();
    clock.0.store(3000, Ordering::SeqCst);
    store
        .insert(chain, &[ext(12, 0, pushed(1, 2, 7))])
        .await
        .unwrap();

    // Newest first, with its recorded time.
    let page1 = store.recent(chain, None, 2).await.unwrap();
    let blocks: Vec<u64> = page1
        .iter()
        .map(|r| r.event.block_number.unwrap())
        .collect();
    assert_eq!(blocks, vec![12, 11]);
    assert_eq!(page1[0].at, 3000);

    // Continue after the last position (exclusive).
    let last = page1.last().unwrap().event.cursor().unwrap();
    let page2 = store.recent(chain, Some(last), 2).await.unwrap();
    let blocks: Vec<u64> = page2
        .iter()
        .map(|r| r.event.block_number.unwrap())
        .collect();
    assert_eq!(blocks, vec![10]);

    // The 24h-style window counts only events recorded at or after the cutoff.
    assert_eq!(store.count_since(chain, 2500).await.unwrap(), 1);
    assert_eq!(store.count_since(chain, 1500).await.unwrap(), 2);
    assert_eq!(store.count_since(chain, 0).await.unwrap(), 3);
}

#[tokio::test]
async fn history_returns_one_strategys_events_in_fold_order() {
    let store = setup().await;
    let chain = ChainId(1);

    // Two strategies, events interleaved across blocks.
    store
        .insert(
            chain,
            &[
                ext(10, 0, shipped(1)),
                ext(10, 1, pushed(1, 2, 1000)),
                ext(11, 0, shipped(2)), // a different strategy
                ext(11, 1, pushed(2, 3, 500)),
                ext(12, 0, pushed(1, 3, 250)), // strategy 1, a later block
            ],
        )
        .await
        .unwrap();

    // Only strategy 1's events, in fold order (block/log ascending).
    let history = store
        .history(chain, StrategyHash(B256::from([1; 32])))
        .await
        .unwrap();
    assert_eq!(
        history,
        vec![
            ext(10, 0, shipped(1)),
            ext(10, 1, pushed(1, 2, 1000)),
            ext(12, 0, pushed(1, 3, 250)),
        ]
    );

    // A strategy with no events yields an empty history.
    let none = store
        .history(chain, StrategyHash(B256::from([9; 32])))
        .await
        .unwrap();
    assert!(none.is_empty());
}
