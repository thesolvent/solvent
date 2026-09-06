//! SQLite trade store integration test — hermetic: an in-memory database, no external service, so it
//! runs on the default `cargo test`. Covers the create → settle lifecycle, order-hash idempotency,
//! the monotonic advance guard, terminal-settle idempotency, and filtered/paginated listing.

use alloy::primitives::{Address, Bytes, B256, U256};
use solvent_adapters::trade::SqliteTradeStore;
use solvent_core::{
    deps::trade::{Page, Settlement, TradeFilter, TradeStore},
    primitives::{
        trade::{Trade, TradeAttempt, TradeId, TradeLeg, TradeStatus},
        IntentId, MakerId, StrategyHash,
    },
};
use sqlx::sqlite::SqlitePoolOptions;
use ulid::Ulid;

/// A migrated, empty in-memory store. One connection keeps the `:memory:` database alive.
async fn setup() -> SqliteTradeStore {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    let store = SqliteTradeStore::new(pool);
    store.migrate().await.expect("migrate");
    store
}

/// A deterministic, strictly-increasing trade id (ULID timestamp = `i`, so `ORDER BY id` is stable).
fn tid(i: u64) -> TradeId {
    TradeId(Ulid::from_parts(1_700_000_000_000 + i, u128::from(i)))
}

fn trade(id: TradeId, order: u8, taker: u8, status: TradeStatus) -> Trade {
    Trade {
        id,
        order_hash: IntentId(B256::from([order; 32])),
        taker: Address::from([taker; 20]),
        token_in: Address::from([1; 20]),
        token_out: Address::from([2; 20]),
        amount_in: U256::from(1_000u64),
        min_amount_out: U256::from(900u64),
        amount_out: None,
        status,
        deadline_block: 100,
        signature: Some(Bytes::from(vec![0xaa, 0xbb])),
        price_impact_pct: Some(0.42),
        surplus: Some(U256::from(50u64)),
        tx_hash: None,
        block_number: None,
        created_at: 1_700_000_000,
        settled_at: None,
    }
}

fn leg(maker: u8, amount_out: u64) -> TradeLeg {
    TradeLeg {
        maker: MakerId(Address::from([maker; 20])),
        strategy_hash: StrategyHash(B256::from([maker; 32])),
        amount_in: U256::from(500u64),
        amount_out: U256::from(amount_out),
    }
}

fn created(at: u64) -> Vec<TradeAttempt> {
    vec![TradeAttempt {
        status: TradeStatus::Created,
        at,
    }]
}

#[tokio::test]
async fn create_then_info_round_trips() {
    let store = setup().await;
    let id = tid(1);
    let t = trade(id, 1, 7, TradeStatus::Reserved);
    let legs = vec![leg(3, 600), leg(4, 400)];
    let attempts = vec![
        TradeAttempt {
            status: TradeStatus::Created,
            at: 1_700_000_000,
        },
        TradeAttempt {
            status: TradeStatus::Quoted,
            at: 1_700_000_001,
        },
        TradeAttempt {
            status: TradeStatus::Reserved,
            at: 1_700_000_002,
        },
    ];

    let res = store.create(&t, &legs, &attempts).await.unwrap();
    assert!(res.created);
    assert_eq!(res.id, id);

    let info = store.info(&id).await.unwrap().expect("present");
    assert_eq!(info.trade, t);
    assert_eq!(info.legs, legs);
    assert_eq!(info.attempts.len(), 3);
    assert_eq!(info.attempts[0].status, TradeStatus::Created);
    assert_eq!(info.attempts[2].status, TradeStatus::Reserved);
}

#[tokio::test]
async fn create_is_idempotent_on_order_hash() {
    let store = setup().await;
    let first = trade(tid(1), 5, 7, TradeStatus::Reserved);
    store
        .create(&first, &[leg(3, 600)], &created(1_700_000_000))
        .await
        .unwrap();

    // A resubmit of the same order (fresh ULID, different body) returns the existing id and writes
    // nothing new.
    let again = trade(tid(2), 5, 7, TradeStatus::Reserved);
    let res = store
        .create(&again, &[leg(9, 1)], &created(1_700_000_009))
        .await
        .unwrap();
    assert!(!res.created);
    assert_eq!(res.id, first.id);

    let info = store.info(&first.id).await.unwrap().expect("present");
    assert_eq!(info.legs, vec![leg(3, 600)]);
    assert!(store.info(&again.id).await.unwrap().is_none());
}

#[tokio::test]
async fn advance_moves_forward_only() {
    let store = setup().await;
    let id = tid(1);
    store
        .create(
            &trade(id, 1, 7, TradeStatus::Reserved),
            &[leg(3, 600)],
            &created(1_700_000_000),
        )
        .await
        .unwrap();

    store
        .advance(&id, TradeStatus::Submitted, 1_700_000_005)
        .await
        .unwrap();
    assert_eq!(
        store.info(&id).await.unwrap().unwrap().trade.status,
        TradeStatus::Submitted
    );

    // A backward advance is ignored; a duplicate does not duplicate the attempt.
    store
        .advance(&id, TradeStatus::Reserved, 1_700_000_006)
        .await
        .unwrap();
    store
        .advance(&id, TradeStatus::Submitted, 1_700_000_007)
        .await
        .unwrap();

    let info = store.info(&id).await.unwrap().unwrap();
    assert_eq!(info.trade.status, TradeStatus::Submitted);
    let submitted = info
        .attempts
        .iter()
        .filter(|a| a.status == TradeStatus::Submitted)
        .count();
    assert_eq!(submitted, 1);
}

#[tokio::test]
async fn settle_is_terminal_and_idempotent() {
    let store = setup().await;
    let id = tid(1);
    store
        .create(
            &trade(id, 1, 7, TradeStatus::Submitted),
            &[leg(3, 600)],
            &created(1_700_000_000),
        )
        .await
        .unwrap();

    store
        .settle(
            &id,
            &Settlement {
                status: TradeStatus::Confirmed,
                amount_out: Some(U256::from(950u64)),
                tx_hash: Some(B256::from([9; 32])),
                block_number: Some(123),
                at: 1_700_000_010,
            },
        )
        .await
        .unwrap();

    let t = store.info(&id).await.unwrap().unwrap().trade;
    assert_eq!(t.status, TradeStatus::Confirmed);
    assert_eq!(t.amount_out, Some(U256::from(950u64)));
    assert_eq!(t.tx_hash, Some(B256::from([9; 32])));
    assert_eq!(t.block_number, Some(123));
    assert_eq!(t.settled_at, Some(1_700_000_010));

    // A second settle (e.g. a reconcile replay) changes nothing.
    store
        .settle(
            &id,
            &Settlement {
                status: TradeStatus::Failed,
                amount_out: None,
                tx_hash: None,
                block_number: None,
                at: 1_700_000_099,
            },
        )
        .await
        .unwrap();
    let t = store.info(&id).await.unwrap().unwrap().trade;
    assert_eq!(t.status, TradeStatus::Confirmed);
    assert_eq!(t.amount_out, Some(U256::from(950u64)));
}

#[tokio::test]
async fn list_filters_and_paginates_newest_first() {
    let store = setup().await;
    for i in 1..=5u64 {
        let taker = if i % 2 == 0 { 8 } else { 7 };
        store
            .create(
                &trade(tid(i), i as u8, taker, TradeStatus::Reserved),
                &[leg(3, 600)],
                &created(1_700_000_000 + i),
            )
            .await
            .unwrap();
    }

    let page1 = store
        .list(
            &TradeFilter::default(),
            &Page {
                limit: 2,
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        page1.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![tid(5), tid(4)]
    );

    let page2 = store
        .list(
            &TradeFilter::default(),
            &Page {
                limit: 2,
                cursor: Some(tid(4)),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        page2.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![tid(3), tid(2)]
    );

    let taker8 = store
        .list(
            &TradeFilter {
                taker: Some(Address::from([8; 20])),
                ..Default::default()
            },
            &Page {
                limit: 10,
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        taker8.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![tid(4), tid(2)]
    );

    let confirmed = store
        .list(
            &TradeFilter {
                status: Some(TradeStatus::Confirmed),
                ..Default::default()
            },
            &Page {
                limit: 10,
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert!(confirmed.is_empty());

    // The pair filter is order-independent — reversed tokens still match all five.
    let pair = store
        .list(
            &TradeFilter {
                pair: Some((Address::from([2; 20]), Address::from([1; 20]))),
                ..Default::default()
            },
            &Page {
                limit: 10,
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(pair.len(), 5);
}

#[tokio::test]
async fn stats_counts_settled_confirmed_and_median_impact() {
    let store = setup().await;
    // Four trades; every trade() carries price_impact_pct = 0.42.
    for (i, order, taker) in [(0u64, 1u8, 1u8), (1, 2, 1), (2, 3, 2), (3, 4, 2)] {
        store
            .create(
                &trade(tid(i), order, taker, TradeStatus::Created),
                &[],
                &created(1),
            )
            .await
            .unwrap();
    }
    let settlement = |status| Settlement {
        status,
        amount_out: Some(U256::from(900u64)),
        tx_hash: None,
        block_number: None,
        at: 1_700_000_100,
    };
    // Settle two confirmed, one failed; leave the fourth open.
    store
        .settle(&tid(0), &settlement(TradeStatus::Confirmed))
        .await
        .unwrap();
    store
        .settle(&tid(1), &settlement(TradeStatus::Confirmed))
        .await
        .unwrap();
    store
        .settle(&tid(2), &settlement(TradeStatus::Failed))
        .await
        .unwrap();

    let stats = store.stats().await.unwrap();
    assert_eq!(stats.settled, 3, "three trades reached a terminal state");
    assert_eq!(stats.confirmed, 2, "two of them confirmed");
    assert_eq!(stats.failed, 1, "one of them failed");
    assert_eq!(stats.median_impact_pct, Some(0.42));
}
