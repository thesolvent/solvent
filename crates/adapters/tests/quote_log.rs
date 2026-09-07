//! SQLite quote-log integration test — hermetic (in-memory SQLite, no external service). Covers the
//! one-transaction write of a served quote and its participants, and a no-route quote (event, no
//! participants).

use std::sync::Arc;

use alloy::primitives::{Address, B256};
use solvent_adapters::metrics::SqliteQuoteLog;
use solvent_core::deps::ledger::Clock;
use solvent_core::deps::quote_log::{QuoteLog, QuoteParticipant, QuoteServed};
use solvent_core::primitives::registry::TokenPair;
use solvent_core::primitives::{ChainId, MakerId, StrategyHash};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

struct FixedClock(u64);
impl Clock for FixedClock {
    fn now_unix(&self) -> u64 {
        self.0
    }
}

fn addr(n: u8) -> Address {
    Address::from([n; 20])
}

/// A migrated in-memory log plus its pool (for read-back). One connection keeps `:memory:` alive.
async fn setup() -> (SqlitePool, SqliteQuoteLog) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    let log = SqliteQuoteLog::new(pool.clone(), Arc::new(FixedClock(1000)));
    log.migrate().await.expect("migrate");
    (pool, log)
}

#[tokio::test]
async fn records_a_quote_with_its_participants() {
    let (pool, log) = setup().await;
    log.record(&QuoteServed {
        chain_id: ChainId(31337),
        pair: TokenPair::new(addr(1), addr(2)),
        latency_ms: 42,
        participants: vec![
            QuoteParticipant {
                maker: MakerId(addr(3)),
                strategy_hash: StrategyHash(B256::from([7; 32])),
            },
            QuoteParticipant {
                maker: MakerId(addr(4)),
                strategy_hash: StrategyHash(B256::from([8; 32])),
            },
        ],
    })
    .await
    .expect("record");

    let (served_at, latency): (i64, i64) =
        sqlx::query_as("SELECT served_at, latency_ms FROM quote_event")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(served_at, 1000, "stamped by the clock");
    assert_eq!(latency, 42);

    let participants: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM quote_participant")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(participants, 2);
}

#[tokio::test]
async fn records_a_no_route_quote_with_no_participants() {
    let (pool, log) = setup().await;
    log.record(&QuoteServed {
        chain_id: ChainId(31337),
        pair: TokenPair::new(addr(1), addr(2)),
        latency_ms: 5,
        participants: vec![],
    })
    .await
    .expect("record");

    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM quote_event")
        .fetch_one(&pool)
        .await
        .unwrap();
    let participants: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM quote_participant")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((events, participants), (1, 0));
}
