//! The live order feed against a local stand-in for Uniswap's Orders API.
//!
//! The mirror serves the same shape the real endpoint does, replaying the captured mainnet page in
//! `fixtures/mainnet/`. That is also how the fork harness will feed this client, so the code under
//! test here is the code that runs in production — only the base URL differs.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::address;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::{json, Value};
use solvent_adapters::ingest::uniswapx::{
    FeedHealth, HostedFeed, OrdersApiClient, Scope, UniswapXV2Normalizer,
};
use solvent_core::deps::ingest::Normalizer;
use solvent_core::deps::ingest::OrderFeed;
use solvent_core::primitives::ingest::RawOrder;
use solvent_core::primitives::ChainId;
use tokio::net::TcpListener;

const POLL: Duration = Duration::from_millis(20);

/// What the mirror should do with each request, so a test can drive the failure paths.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    Serve,
    /// The edge rejecting an unfamiliar client, which is what a missing user agent earns.
    Refuse,
    RateLimit,
}

struct Mirror {
    behaviour: Behaviour,
    hits: Arc<AtomicUsize>,
    saw_user_agent: Arc<AtomicUsize>,
    last_query: Arc<parking_lot::Mutex<Vec<(String, String)>>>,
}

/// The captured mainnet page, served verbatim.
fn page() -> Value {
    let corpus: Value = serde_json::from_str(include_str!("fixtures/mainnet/dutch_v2_orders.json"))
        .expect("corpus parses");
    let orders: Vec<Value> = corpus["orders"]
        .as_array()
        .expect("orders array")
        .iter()
        .map(|o| {
            json!({
                "orderHash": o["orderHash"],
                "encodedOrder": o["encodedOrder"],
                "signature": o["signature"],
                "createdAt": o["createdAt"],
                "orderStatus": "open",
                "type": "Dutch_V2",
                "chainId": 1,
            })
        })
        .collect();
    json!({ "orders": orders })
}

fn corpus_len() -> usize {
    page()["orders"].as_array().expect("orders").len()
}

async fn handler(
    State(mirror): State<Arc<Mirror>>,
    Query(params): Query<Vec<(String, String)>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    mirror.hits.fetch_add(1, Ordering::SeqCst);
    if headers.contains_key("user-agent") {
        mirror.saw_user_agent.fetch_add(1, Ordering::SeqCst);
    }
    *mirror.last_query.lock() = params;
    match mirror.behaviour {
        Behaviour::Serve => Ok(Json(page())),
        Behaviour::Refuse => Err(StatusCode::FORBIDDEN),
        Behaviour::RateLimit => Err(StatusCode::TOO_MANY_REQUESTS),
    }
}

/// A running mirror plus the counters a test asserts on.
struct Harness {
    base: String,
    hits: Arc<AtomicUsize>,
    saw_user_agent: Arc<AtomicUsize>,
    last_query: Arc<parking_lot::Mutex<Vec<(String, String)>>>,
}

async fn spawn(behaviour: Behaviour) -> Harness {
    let hits = Arc::new(AtomicUsize::new(0));
    let saw_user_agent = Arc::new(AtomicUsize::new(0));
    let last_query = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let mirror = Arc::new(Mirror {
        behaviour,
        hits: Arc::clone(&hits),
        saw_user_agent: Arc::clone(&saw_user_agent),
        last_query: Arc::clone(&last_query),
    });
    let app = Router::new()
        .route("/orders", get(handler))
        .with_state(mirror);
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("binds");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Harness {
        base: format!("http://{addr}"),
        hits,
        saw_user_agent,
        last_query,
    }
}

fn feed(base: &str, scopes: Vec<Scope>, health: Arc<FeedHealth>) -> HostedFeed {
    let client = OrdersApiClient::new(base.to_string(), ChainId(1), "Dutch_V2".to_string())
        .expect("client builds");
    HostedFeed::new(Arc::new(client), ChainId(1), scopes, POLL, health)
}

/// Pull `n` orders off the stream, or give up rather than hang if the feed stalls.
async fn take(feed: &HostedFeed, n: usize) -> Vec<RawOrder> {
    tokio::time::timeout(Duration::from_secs(5), feed.stream().take(n).collect())
        .await
        .expect("feed produced orders before the timeout")
}

#[tokio::test]
async fn streams_every_order_on_the_page() {
    let harness = spawn(Behaviour::Serve).await;
    let health = Arc::new(FeedHealth::default());
    let feed = feed(&harness.base, vec![Scope::Book], Arc::clone(&health));

    let orders = take(&feed, corpus_len()).await;
    assert_eq!(orders.len(), corpus_len());
    assert!(orders.iter().all(|o| !o.payload.is_empty()));
    assert!(orders.iter().all(|o| o.chain == ChainId(1)));
    assert!(health.is_live(now()));
}

/// The endpoint's edge refuses clients it does not recognise, and a missing user agent is the usual
/// cause. Every request must carry one.
#[tokio::test]
async fn every_request_identifies_this_client() {
    let harness = spawn(Behaviour::Serve).await;
    let feed = feed(
        &harness.base,
        vec![Scope::Book],
        Arc::new(FeedHealth::default()),
    );
    take(&feed, corpus_len()).await;

    let hits = harness.hits.load(Ordering::SeqCst);
    assert!(hits >= 1);
    assert_eq!(harness.saw_user_agent.load(Ordering::SeqCst), hits);
}

/// The query is pinned: the endpoint 400s on `cursor` and on any non-default ordering, so the
/// client must never grow paging, and it must ask for the order type it can actually decode.
#[tokio::test]
async fn the_query_asks_only_for_what_the_endpoint_supports() {
    let harness = spawn(Behaviour::Serve).await;
    let feed = feed(
        &harness.base,
        vec![Scope::Book],
        Arc::new(FeedHealth::default()),
    );
    take(&feed, 1).await;

    let query = harness.last_query.lock().clone();
    let has = |k: &str, v: &str| query.iter().any(|(a, b)| a == k && b == v);
    assert!(has("chainId", "1"));
    assert!(has("orderStatus", "open"));
    assert!(has("orderType", "Dutch_V2"));
    for unsupported in ["cursor", "sort", "desc", "pair"] {
        assert!(
            !query.iter().any(|(k, _)| k == unsupported),
            "{unsupported} is rejected by the endpoint and must not be sent"
        );
    }
}

/// Orders assigned to us are polled on their own so their discovery never queues behind the wider
/// book. Both scopes share one request budget, alternating.
#[tokio::test]
async fn both_scopes_are_polled_over_one_budget() {
    let harness = spawn(Behaviour::Serve).await;
    let filler = address!("1111111111111111111111111111111111111111");
    let feed = feed(
        &harness.base,
        vec![Scope::ExclusiveTo(filler), Scope::Book],
        Arc::new(FeedHealth::default()),
    );

    // Two full pages spans at least two requests, so both scopes have had a turn.
    take(&feed, corpus_len() * 2).await;
    assert!(harness.hits.load(Ordering::SeqCst) >= 2);
}

/// A refusal is a policy decision at the edge: retrying unchanged will keep failing, so readiness
/// fails at once rather than after a silence budget. The stream must still stay open — a feed that
/// ends is a filler that has silently stopped.
#[tokio::test]
async fn a_refusal_fails_readiness_without_ending_the_stream() {
    let harness = spawn(Behaviour::Refuse).await;
    let health = Arc::new(FeedHealth::default());
    let feed = feed(&harness.base, vec![Scope::Book], Arc::clone(&health));

    let mut stream = feed.stream();
    let idle = tokio::time::timeout(Duration::from_millis(200), stream.next()).await;
    assert!(idle.is_err(), "a refused poll yields no orders");
    assert!(harness.hits.load(Ordering::SeqCst) >= 1, "and keeps trying");
    assert!(!health.is_live(now()), "readiness fails immediately");
}

/// A rate limit is transient. It must not fail readiness on its own, and the feed must keep polling.
#[tokio::test]
async fn a_rate_limit_is_transient_and_keeps_the_feed_ready() {
    let harness = spawn(Behaviour::RateLimit).await;
    let health = Arc::new(FeedHealth::default());
    let feed = feed(&harness.base, vec![Scope::Book], Arc::clone(&health));

    let mut stream = feed.stream();
    let _ = tokio::time::timeout(Duration::from_millis(200), stream.next()).await;
    assert!(harness.hits.load(Ordering::SeqCst) >= 2, "keeps polling");
    assert!(health.is_live(now()));
    assert!(health.consecutive_failures() >= 1);
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Against the real endpoint. Ignored by default: it needs the network, and what it returns depends
/// on live traffic. Run with `cargo test -p solvent-adapters --test hosted_feed live_ -- --ignored
/// --nocapture` to confirm the client still speaks to the deployment as configured.
///
/// An empty page is a pass. Open orders are filled within seconds on mainnet, so the book is
/// usually empty at any given instant; what this proves is that the request is shaped correctly and
/// the edge accepts this client.
#[tokio::test]
#[ignore]
async fn live_endpoint_accepts_this_client() {
    let client = OrdersApiClient::new(
        "https://api.uniswap.org/v2".to_string(),
        ChainId(1),
        "Dutch_V2".to_string(),
    )
    .expect("client builds");

    let orders = client
        .open_orders(Scope::Book)
        .await
        .expect("the live endpoint accepts this client");
    println!("live open Dutch_V2 orders on mainnet: {}", orders.len());

    // The full chain, on orders nobody staged for us: fetch, convert, decode, verify against the
    // live cosigner, and report each verdict.
    let normalizer = UniswapXV2Normalizer::new(
        address!("00000011F84B9aa48e5f8aA8B9897600006289Be"),
        vec![address!("4449Cd34d1eb1FEDCF02A1Be3834FfDe8E6A6180")],
    );
    for record in &orders {
        let raw = record.to_raw_order(ChainId(1)).expect("record converts");
        match normalizer.normalize(&raw) {
            Ok(intent) => {
                assert_eq!(
                    format!("{:#x}", intent.id.0),
                    record.order_hash.to_lowercase(),
                    "our order hash must match the one the API published"
                );
                println!(
                    "  {} accepted: {} outputs, exclusive={}, {} bytes",
                    &record.order_hash[..14],
                    intent.outputs.len(),
                    intent.exclusivity.is_some(),
                    raw.payload.len()
                );
            }
            Err(e) => println!("  {} declined: {e}", &record.order_hash[..14]),
        }
    }
}

/// Watches the live endpoint through the real feed for a while, running every order through the
/// real normalizer and reporting the verdict. Ignored: it needs the network and takes a minute.
///
/// `cargo test -p solvent-adapters --test hosted_feed live_watch -- --ignored --nocapture`
#[tokio::test]
#[ignore]
async fn live_watch_the_book() {
    let health = Arc::new(FeedHealth::default());
    let feed = {
        let client = OrdersApiClient::new(
            "https://api.uniswap.org/v2".to_string(),
            ChainId(1),
            "Dutch_V2".to_string(),
        )
        .expect("client builds");
        HostedFeed::new(
            Arc::new(client),
            ChainId(1),
            vec![Scope::Book],
            Duration::from_millis(250),
            Arc::clone(&health),
        )
    };
    let normalizer = UniswapXV2Normalizer::new(
        address!("00000011F84B9aa48e5f8aA8B9897600006289Be"),
        vec![address!("4449Cd34d1eb1FEDCF02A1Be3834FfDe8E6A6180")],
    );

    let watch = Duration::from_secs(45);
    let mut seen: std::collections::BTreeSet<String> = Default::default();
    let mut accepted = 0usize;
    let mut declined = 0usize;
    let mut stream = feed.stream();

    println!("watching mainnet for {}s ...", watch.as_secs());
    let deadline = tokio::time::Instant::now() + watch;
    while let Ok(Some(raw)) = tokio::time::timeout_at(deadline, stream.next()).await {
        match normalizer.normalize(&raw) {
            Ok(intent) => {
                let key = format!("{:#x}", intent.id.0);
                if !seen.insert(key.clone()) {
                    continue;
                }
                accepted += 1;
                let ex = intent
                    .exclusivity
                    .map(|e| format!("{} (+{}bps until {})", e.filler, e.override_bps, e.ends_at))
                    .unwrap_or_else(|| "OPEN".to_string());
                println!(
                    "  ACCEPT {}  in={} outs={} deadline={} reserved-for={}",
                    &key[..14],
                    intent.input.token,
                    intent.outputs.len(),
                    intent.deadline,
                    ex
                );
            }
            Err(e) => {
                declined += 1;
                println!("  DECLINE {e}");
            }
        }
    }
    println!(
        "\n{} distinct orders accepted, {} declined, feed live={}",
        accepted,
        declined,
        health.is_live(now())
    );
}
