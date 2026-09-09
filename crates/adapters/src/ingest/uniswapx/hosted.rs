//! The live order feed: polls Uniswap's Orders API and streams what it finds.
//!
//! Two scopes share one request budget. Orders already assigned to us are polled on their own, so
//! discovering them never queues behind a page of the wider book — the exclusivity window is ~24s
//! and every round trip spends from it. The wider book is where an unassigned order is found.
//!
//! Duplicates are expected, not exceptional: each poll returns the same newest page, so an order
//! appears on every tick until it is filled or expires. The pipeline's dedup absorbs that, which is
//! why this feed does not keep its own.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{BoxStream, StreamExt};

use solvent_core::deps::ingest::OrderFeed;
use solvent_core::primitives::ingest::RawOrder;
use solvent_core::primitives::ChainId;

use tracing::{debug, warn};

use super::orders_api::{OrdersApiClient, OrdersApiError, Scope};

/// How long the feed may see nothing before [`FeedHealth::is_live`] turns false. Generous: an empty
/// book is normal, and the signal is meant to catch a feed that has stopped working, not a quiet
/// market.
const DEFAULT_SILENCE_BUDGET: Duration = Duration::from_secs(300);

/// Whether the feed is still reaching the API, shared with whatever reports readiness.
///
/// The failure this exists for is silent: an edge policy changes, every request comes back `403`,
/// and the process keeps running with an empty order stream while looking healthy. A filler that
/// cannot see orders is down, and should say so.
#[derive(Debug)]
pub struct FeedHealth {
    last_success: AtomicU64,
    consecutive_failures: AtomicU64,
    refused: AtomicU64,
    silence_budget_secs: u64,
}

impl FeedHealth {
    pub fn new(silence_budget: Duration) -> FeedHealth {
        FeedHealth {
            last_success: AtomicU64::new(0),
            consecutive_failures: AtomicU64::new(0),
            refused: AtomicU64::new(0),
            silence_budget_secs: silence_budget.as_secs(),
        }
    }

    /// A poll that reached the API, whether or not it carried orders.
    fn record_success(&self, now: u64) {
        self.last_success.store(now, Ordering::Relaxed);
        self.consecutive_failures.store(0, Ordering::Relaxed);
        self.refused.store(0, Ordering::Relaxed);
    }

    fn record_failure(&self, refused: bool) {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
        if refused {
            self.refused.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// False once the API has been unreachable for longer than the silence budget, or as soon as it
    /// has refused us outright — a refusal will not heal on its own, so there is nothing to wait for.
    pub fn is_live(&self, now: u64) -> bool {
        if self.refused.load(Ordering::Relaxed) > 0 {
            return false;
        }
        let last = self.last_success.load(Ordering::Relaxed);
        // Before the first poll lands there is nothing to have gone stale.
        last == 0 || now.saturating_sub(last) <= self.silence_budget_secs
    }

    pub fn consecutive_failures(&self) -> u64 {
        self.consecutive_failures.load(Ordering::Relaxed)
    }
}

impl Default for FeedHealth {
    fn default() -> FeedHealth {
        FeedHealth::new(DEFAULT_SILENCE_BUDGET)
    }
}

/// Polls the Orders API forever, yielding every order it sees.
pub struct HostedFeed {
    client: Arc<OrdersApiClient>,
    chain: ChainId,
    scopes: Vec<Scope>,
    /// Gap between individual requests. This *is* the rate limit: the endpoint allows 4 per second,
    /// and one request leaves per interval regardless of how many scopes are configured.
    interval: Duration,
    health: Arc<FeedHealth>,
}

impl HostedFeed {
    pub fn new(
        client: Arc<OrdersApiClient>,
        chain: ChainId,
        scopes: Vec<Scope>,
        interval: Duration,
        health: Arc<FeedHealth>,
    ) -> HostedFeed {
        HostedFeed {
            client,
            chain,
            scopes,
            interval,
            health,
        }
    }

    pub fn health(&self) -> Arc<FeedHealth> {
        Arc::clone(&self.health)
    }
}

impl OrderFeed for HostedFeed {
    fn stream(&self) -> BoxStream<'static, RawOrder> {
        let client = Arc::clone(&self.client);
        let health = Arc::clone(&self.health);
        let chain = self.chain;
        let scopes = self.scopes.clone();
        let interval = self.interval;

        // One request per tick, cycling through the scopes, so adding a scope costs latency rather
        // than exceeding the shared budget.
        let ticks = futures::stream::unfold(0usize, move |turn| {
            let client = Arc::clone(&client);
            let health = Arc::clone(&health);
            let scopes = scopes.clone();
            async move {
                if scopes.is_empty() {
                    return None;
                }
                tokio::time::sleep(interval).await;
                let scope = scopes[turn % scopes.len()];
                let orders = poll_once(&client, &health, chain, scope).await;
                Some((futures::stream::iter(orders), turn.wrapping_add(1)))
            }
        });
        ticks.flatten().boxed()
    }
}

/// One request. Never fails the stream: a poll that errors is logged, recorded against health, and
/// retried on the next tick, because a feed that ends is a filler that has silently stopped.
async fn poll_once(
    client: &OrdersApiClient,
    health: &FeedHealth,
    chain: ChainId,
    scope: Scope,
) -> Vec<RawOrder> {
    match client.open_orders(scope).await {
        Ok(records) => {
            health.record_success(now_unix());
            records
                .iter()
                .filter_map(|record| match record.to_raw_order(chain) {
                    Ok(raw) => Some(raw),
                    Err(e) => {
                        warn!("orders feed: unusable record {}: {}", record.order_hash, e);
                        None
                    }
                })
                .collect()
        }
        Err(OrdersApiError::Refused) => {
            health.record_failure(true);
            warn!("orders feed refused by the endpoint; the order stream is down");
            Vec::new()
        }
        Err(e) => {
            health.record_failure(false);
            debug!("orders feed poll failed: {}", e);
            Vec::new()
        }
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_starts_live_before_the_first_poll() {
        let health = FeedHealth::new(Duration::from_secs(60));
        assert!(health.is_live(1_000));
    }

    #[test]
    fn health_goes_stale_after_the_silence_budget() {
        let health = FeedHealth::new(Duration::from_secs(60));
        health.record_success(1_000);
        assert!(health.is_live(1_060), "inside the budget");
        assert!(!health.is_live(1_061), "past the budget");
    }

    /// An empty book still counts as reaching the API. A quiet market must not read as an outage.
    #[test]
    fn an_empty_poll_keeps_the_feed_live() {
        let health = FeedHealth::new(Duration::from_secs(60));
        health.record_success(1_000);
        health.record_success(1_030);
        assert!(health.is_live(1_080));
    }

    /// A refusal is a policy decision at the edge, not a transient fault: it does not heal by
    /// waiting, so readiness fails immediately rather than after the silence budget.
    #[test]
    fn a_refusal_fails_readiness_at_once() {
        let health = FeedHealth::new(Duration::from_secs(3_600));
        health.record_success(1_000);
        health.record_failure(true);
        assert!(!health.is_live(1_001));
    }

    #[test]
    fn a_transient_failure_does_not_fail_readiness_on_its_own() {
        let health = FeedHealth::new(Duration::from_secs(60));
        health.record_success(1_000);
        health.record_failure(false);
        assert!(health.is_live(1_010));
        assert_eq!(health.consecutive_failures(), 1);
    }

    #[test]
    fn a_success_clears_earlier_failures() {
        let health = FeedHealth::new(Duration::from_secs(60));
        health.record_failure(true);
        health.record_failure(false);
        health.record_success(2_000);
        assert_eq!(health.consecutive_failures(), 0);
        assert!(health.is_live(2_001));
    }
}
