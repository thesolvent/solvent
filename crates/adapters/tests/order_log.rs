//! The observation log round-trips: what the writer stores is what the reader returns.
//!
//! Worth a test because the schema, the insert and the select are three separate lists of columns
//! that only agree by hand — and because nothing else in the suite would notice if the log quietly
//! recorded nothing at all.

use alloy::primitives::{Address, Bytes, B256, U256};
use solvent_adapters::metrics::SqliteOrderLog;
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

    let rows = log.recent(10).await.expect("read back");
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
    assert_eq!(log.recent(10).await.expect("read").len(), 1);
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

    let row = log.recent(1).await.expect("read").pop().expect("one row");
    assert_eq!(row.indicative_in.as_deref(), Some("2240877102"));
}
