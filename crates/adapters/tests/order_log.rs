//! The observation log round-trips: what the writer stores is what the reader returns.
//!
//! Worth a test because the schema, the insert and the select are three separate lists of columns
//! that only agree by hand — and because nothing else in the suite would notice if the log quietly
//! recorded nothing at all.

use alloy::primitives::{Address, Bytes, B256, U256};
use solvent_adapters::metrics::{OrderFeedFilter, OrderStateFilter, SqliteOrderLog};
use solvent_core::deps::order_log::{OrderLog, OrderObserved, OrderVerdict};
use solvent_core::primitives::ingest::{
    AmountCurve, Intent, IntentInput, IntentOutput, IntentParts, OrderSource, ProtocolId,
};
use solvent_core::primitives::{ChainId, IntentId};
use sqlx::SqlitePool;

fn addr(n: u8) -> Address {
    Address::repeat_byte(n)
}

fn intent(tag: u8, amount_in: u64, out: u64) -> Intent {
    Intent::new(IntentParts {
        deadline: 2_000_000_000,
        raw: Bytes::new(),
        signature: Bytes::new(),
        observed_at: 1_000,
        source: OrderSource::UniswapX,
        ..IntentParts::new(
            IntentId(B256::repeat_byte(tag)),
            ProtocolId::UniswapXV2,
            addr(9),
            IntentInput::new(addr(1), AmountCurve::scalar(U256::from(amount_in))),
            vec![IntentOutput::new(
                addr(2),
                AmountCurve::scalar(U256::from(out)),
                addr(9),
            )],
            ChainId(1),
        )
    })
}

async fn log() -> SqliteOrderLog {
    let pool = SqlitePool::connect("sqlite::memory:").await.expect("open");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrate");
    SqliteOrderLog::new(pool)
}

#[tokio::test]
async fn an_admitted_and_a_refused_order_both_come_back() {
    let log = log().await;
    let admitted = intent(1, 1_000, 500);
    let refused = intent(2, 7_000, 900);

    for (intent, verdict) in [
        (&admitted, OrderVerdict::Admitted),
        (&refused, OrderVerdict::Dropped("native currency leg")),
    ] {
        log.record(&OrderObserved {
            intent,
            source: intent.source,
            verdict,
            seen_at: 1_000,
        })
        .await
        .expect("record");
    }

    let rows = log
        .recent(10, 0, &OrderFeedFilter::default())
        .await
        .expect("read back");
    assert_eq!(
        rows.len(),
        2,
        "both verdicts are recorded, not just the fill"
    );

    let refused_row = rows
        .iter()
        .find(|r| r.verdict == "dropped")
        .expect("the refused order is in the log");
    assert_eq!(refused_row.reason.as_deref(), Some("native currency leg"));
    assert_eq!(refused_row.source, "uniswapx");
    assert_eq!(refused_row.amount_in, "7000");
    assert_eq!(refused_row.required_out.as_deref(), Some("900"));
    assert_eq!(
        refused_row.order_hash,
        format!("0x{}", "02".repeat(32)),
        "the hash round-trips as hex, not raw bytes"
    );

    let admitted_row = rows
        .iter()
        .find(|r| r.verdict == "admitted")
        .expect("the admitted order is in the log");
    assert_eq!(
        admitted_row.reason, None,
        "an admission has no refusing rule"
    );
    assert_eq!(admitted_row.indicative_in, None, "not priced until routed");
}

/// The same order arriving on a later poll must not double-count the denominator.
#[tokio::test]
async fn the_same_order_is_recorded_once() {
    let log = log().await;
    let order = intent(3, 1_000, 500);
    for _ in 0..3 {
        log.record(&OrderObserved {
            intent: &order,
            source: order.source,
            verdict: OrderVerdict::Admitted,
            seen_at: 1_000,
        })
        .await
        .expect("record");
    }
    assert_eq!(
        log.recent(10, 0, &OrderFeedFilter::default())
            .await
            .expect("read")
            .len(),
        1
    );
}

/// What routing would have cost is only known after admission, so it lands by a later update —
/// this is the number that turns "refused" into "refused, and by how much".
#[tokio::test]
async fn the_sourcing_cost_attaches_after_routing() {
    let log = log().await;
    let order = intent(4, 2_000, 800);
    log.record(&OrderObserved {
        intent: &order,
        source: order.source,
        verdict: OrderVerdict::Admitted,
        seen_at: 1_000,
    })
    .await
    .expect("record");

    log.record_quote(order.id, "2240877102")
        .await
        .expect("quote");

    let row = log
        .recent(1, 0, &OrderFeedFilter::default())
        .await
        .expect("read")
        .pop()
        .expect("one row");
    assert_eq!(row.indicative_in.as_deref(), Some("2240877102"));
}

/// `offset` pages through the feed newest-first without skipping or repeating a row, and `count`
/// reports the true total regardless of what page `recent` is asked for.
#[tokio::test]
async fn recent_pages_by_offset_and_count_reports_the_true_total() {
    let log = log().await;
    for tag in 0..5u8 {
        let order = intent(tag, 1_000, 500);
        log.record(&OrderObserved {
            intent: &order,
            source: order.source,
            verdict: OrderVerdict::Admitted,
            seen_at: 1_000 + u64::from(tag),
        })
        .await
        .expect("record");
    }

    assert_eq!(
        log.count(&OrderFeedFilter::default()).await.expect("count"),
        5
    );

    let page1 = log
        .recent(2, 0, &OrderFeedFilter::default())
        .await
        .expect("page1");
    let page2 = log
        .recent(2, 2, &OrderFeedFilter::default())
        .await
        .expect("page2");
    assert_eq!(page1.len(), 2);
    assert_eq!(page2.len(), 2);
    // Newest first, no overlap: page1 is tags 4,3; page2 is tags 2,1.
    let hash = |tag: u8| format!("0x{}", alloy::hex::encode(B256::repeat_byte(tag)));
    assert_eq!(
        page1
            .iter()
            .map(|r| r.order_hash.clone())
            .collect::<Vec<_>>(),
        vec![hash(4), hash(3)]
    );
    assert_eq!(
        page2
            .iter()
            .map(|r| r.order_hash.clone())
            .collect::<Vec<_>>(),
        vec![hash(2), hash(1)]
    );
}

/// `source` and `state` narrow the feed the same way the explorer's filter dropdowns do: `state` is
/// not a stored column, it is verdict plus trade lifecycle, so this is the one place that bucketing
/// can silently drift from what the UI actually shows.
#[tokio::test]
async fn recent_filters_by_source_and_derived_state() {
    let pool = SqlitePool::connect("sqlite::memory:").await.expect("open");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrate");
    let log = SqliteOrderLog::new(pool.clone());

    let refused = Intent::new(IntentParts {
        deadline: 2_000_000_000,
        raw: Bytes::new(),
        signature: Bytes::new(),
        observed_at: 1_000,
        source: OrderSource::OneInch,
        ..IntentParts::new(
            IntentId(B256::repeat_byte(1)),
            ProtocolId::UniswapXV2,
            addr(9),
            IntentInput::new(addr(1), AmountCurve::scalar(U256::from(1_000u64))),
            vec![IntentOutput::new(
                addr(2),
                AmountCurve::scalar(U256::from(500u64)),
                addr(9),
            )],
            ChainId(1),
        )
    });
    let filled = intent(2, 1_000, 500);
    let declined = intent(3, 1_000, 500);
    log.record(&OrderObserved {
        intent: &refused,
        source: refused.source,
        verdict: OrderVerdict::Dropped("token not admitted"),
        seen_at: 1_000,
    })
    .await
    .expect("record refused");
    for order in [&filled, &declined] {
        log.record(&OrderObserved {
            intent: order,
            source: order.source,
            verdict: OrderVerdict::Admitted,
            seen_at: 1_000,
        })
        .await
        .expect("record admitted");
    }
    insert_trade(&pool, filled.id, "confirmed").await;
    insert_trade(&pool, declined.id, "declined").await;

    let by_source = OrderFeedFilter {
        source: Some("oneinch".to_string()),
        ..OrderFeedFilter::default()
    };
    let source_rows = log.recent(10, 0, &by_source).await.expect("by source");
    assert_eq!(source_rows.len(), 1);
    assert_eq!(source_rows[0].source, "oneinch");
    assert_eq!(log.count(&by_source).await.expect("count by source"), 1);

    let by_filled = OrderFeedFilter {
        state: Some(OrderStateFilter::Filled),
        ..OrderFeedFilter::default()
    };
    let filled_rows = log.recent(10, 0, &by_filled).await.expect("by filled");
    assert_eq!(filled_rows.len(), 1);
    assert_eq!(filled_rows[0].trade_status.as_deref(), Some("confirmed"));
    assert_eq!(filled_rows[0].trade_id.as_deref(), Some("trade-confirmed"));

    let by_declined = OrderFeedFilter {
        state: Some(OrderStateFilter::Declined),
        ..OrderFeedFilter::default()
    };
    let declined_rows = log.recent(10, 0, &by_declined).await.expect("by declined");
    assert_eq!(declined_rows.len(), 1);
    assert_eq!(declined_rows[0].trade_status.as_deref(), Some("declined"));

    let by_refused = OrderFeedFilter {
        state: Some(OrderStateFilter::Refused),
        ..OrderFeedFilter::default()
    };
    let refused_rows = log.recent(10, 0, &by_refused).await.expect("by refused");
    assert_eq!(refused_rows.len(), 1);
    assert_eq!(refused_rows[0].verdict, "dropped");
}

/// A trade row keyed by `order_hash`, just complete enough to satisfy the schema's `NOT NULL`
/// columns — `recent`'s state filter only reads `status`.
async fn insert_trade(pool: &SqlitePool, order_hash: IntentId, status: &str) {
    sqlx::query(
        "INSERT INTO trade
             (id, order_hash, taker, token_in, token_out, amount_in, min_amount_out,
              status, status_rank, deadline_block, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, 0, 0)",
    )
    .bind(format!("trade-{status}"))
    .bind(order_hash.0.as_slice().to_vec())
    .bind(addr(9).as_slice().to_vec())
    .bind(addr(1).as_slice().to_vec())
    .bind(addr(2).as_slice().to_vec())
    .bind("1000")
    .bind("500")
    .bind(status)
    .execute(pool)
    .await
    .expect("insert trade");
}
