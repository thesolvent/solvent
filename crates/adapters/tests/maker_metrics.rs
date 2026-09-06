//! SQLite maker-metrics integration test — hermetic (in-memory SQLite). Seeds confirmed trades and
//! served quotes, then checks the maker- and position-scoped rollups: fills, per-token volume,
//! fills-by-day bucketing, quotes/latency, and quote uptime.

use alloy::primitives::{Address, B256, U256};
use solvent_adapters::metrics::SqliteMakerMetrics;
use solvent_core::deps::maker_metrics::MakerMetricsStore;
use solvent_core::primitives::registry::TokenPair;
use solvent_core::primitives::{MakerId, StrategyHash};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

const NOW: i64 = 10_000_000;
const DAY: i64 = 86_400;

fn addr(n: u8) -> Address {
    Address::from([n; 20])
}

async fn setup() -> (SqlitePool, SqliteMakerMetrics) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    let metrics = SqliteMakerMetrics::new(pool.clone());
    metrics.migrate().await.expect("migrate");
    (pool, metrics)
}

async fn insert_trade(
    pool: &SqlitePool,
    id: &str,
    order_hash: u8,
    status: &str,
    settled_at: i64,
    token_out: Address,
) {
    sqlx::query(
        "INSERT INTO trade (id, order_hash, taker, token_in, token_out, amount_in, min_amount_out, \
         status, status_rank, deadline_block, created_at, settled_at) \
         VALUES (?, ?, ?, ?, ?, '0', '0', ?, 0, 0, ?, ?)",
    )
    .bind(id)
    .bind(B256::from([order_hash; 32]).as_slice())
    .bind(&[0u8; 20][..])
    .bind(addr(10).as_slice())
    .bind(token_out.as_slice())
    .bind(status)
    .bind(settled_at)
    .bind(settled_at)
    .execute(pool)
    .await
    .expect("insert trade");
}

async fn insert_leg(pool: &SqlitePool, trade_id: &str, maker: u8, strategy: u8, amount_out: &str) {
    sqlx::query(
        "INSERT INTO trade_leg (trade_id, idx, maker, strategy_hash, amount_in, amount_out) \
         VALUES (?, 0, ?, ?, '0', ?)",
    )
    .bind(trade_id)
    .bind(addr(maker).as_slice())
    .bind(B256::from([strategy; 32]).as_slice())
    .bind(amount_out)
    .execute(pool)
    .await
    .expect("insert leg");
}

async fn insert_quote(pool: &SqlitePool, id: &str, latency: i64, maker: u8, strategy: u8) {
    sqlx::query(
        "INSERT INTO quote_event (id, chain_id, pair_lo, pair_hi, served_at, latency_ms) \
         VALUES (?, 1, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(addr(10).as_slice())
    .bind(addr(11).as_slice())
    .bind(NOW - 50)
    .bind(latency)
    .execute(pool)
    .await
    .expect("insert quote_event");
    sqlx::query("INSERT INTO quote_participant (quote_id, maker, strategy_hash) VALUES (?, ?, ?)")
        .bind(id)
        .bind(addr(maker).as_slice())
        .bind(B256::from([strategy; 32]).as_slice())
        .execute(pool)
        .await
        .expect("insert participant");
}

/// Seed: two confirmed fills for maker 1 / strategy 1 (today and 2 days ago), and four quotes on the
/// pair — three sourcing strategy 1, one sourcing a different maker/strategy.
async fn seed(pool: &SqlitePool) {
    insert_trade(pool, "t1", 1, "confirmed", NOW - 100, addr(11)).await;
    insert_leg(pool, "t1", 1, 1, "1000000").await;
    insert_trade(pool, "t2", 2, "confirmed", NOW - 2 * DAY, addr(11)).await;
    insert_leg(pool, "t2", 1, 1, "2000000").await;
    // a non-confirmed trade must not count
    insert_trade(pool, "t3", 3, "failed", NOW - 200, addr(11)).await;
    insert_leg(pool, "t3", 1, 1, "9000000").await;

    insert_quote(pool, "q1", 100, 1, 1).await;
    insert_quote(pool, "q2", 200, 1, 1).await;
    insert_quote(pool, "q3", 300, 1, 1).await;
    insert_quote(pool, "q4", 50, 2, 2).await; // pair quote, strategy 1 absent
}

#[tokio::test]
async fn maker_rollup() {
    let (pool, metrics) = setup().await;
    seed(&pool).await;

    let m = metrics
        .maker(MakerId(addr(1)), (NOW - 7 * DAY) as u64, NOW as u64)
        .await
        .expect("maker");

    assert_eq!(m.fills, 2, "only the two confirmed fills");
    assert_eq!(m.last_fill_at, Some((NOW - 100) as u64));
    assert_eq!(m.quotes, 3, "quotes strategy 1 was sourced into");
    assert_eq!(m.latency_p50_ms, Some(200)); // median of 100/200/300
                                             // volume = 1_000_000 + 2_000_000 of token 11
    let vol = m.volume.iter().find(|v| v.token == addr(11)).unwrap();
    assert_eq!(vol.base_units, U256::from(3_000_000u64));
    // fills land today and 2 days ago
    assert_eq!(m.fills_by_day[6], 1);
    assert_eq!(m.fills_by_day[4], 1);
    assert_eq!(m.fills_by_day.iter().sum::<u64>(), 2);
}

#[tokio::test]
async fn position_rollup_with_uptime() {
    let (pool, metrics) = setup().await;
    seed(&pool).await;

    let p = metrics
        .position(
            StrategyHash(B256::from([1; 32])),
            TokenPair::new(addr(10), addr(11)),
            (NOW - 7 * DAY) as u64,
        )
        .await
        .expect("position");

    assert_eq!(p.fills, 2);
    assert_eq!(p.last_fill_at, Some((NOW - 100) as u64));
    // strategy 1 was in 3 of the 4 pair quotes → 75%.
    assert_eq!(p.quote_uptime_pct, Some(75.0));
    let vol = p.volume.iter().find(|v| v.token == addr(11)).unwrap();
    assert_eq!(vol.base_units, U256::from(3_000_000u64));
}
