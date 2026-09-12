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

use solvent_core::obs::{debug, warn};

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
    /// When the feed first tried and failed, so a never-successful feed can still go stale.
    first_attempt: AtomicU64,
    consecutive_failures: AtomicU64,
    refused: AtomicU64,
    silence_budget_secs: u64,
}

impl FeedHealth {
    pub fn new(silence_budget: Duration) -> FeedHealth {
        FeedHealth {
            last_success: AtomicU64::new(0),
            first_attempt: AtomicU64::new(0),
            consecutive_failures: AtomicU64::new(0),
            refused: AtomicU64::new(0),
            silence_budget_secs: silence_budget.as_secs(),
        }
    }

    /// A poll that reached the API, whether or not it carried orders.
    pub(crate) fn record_success(&self, now: u64) {
        self.last_success.store(now, Ordering::Relaxed);
        self.consecutive_failures.store(0, Ordering::Relaxed);
        self.refused.store(0, Ordering::Relaxed);
    }

    pub(crate) fn record_failure(&self, now: u64, refused: bool) {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
        // The first attempt anchors the silence budget when no poll has ever succeeded; without it
        // a feed whose very first poll fails has no `last_success` to go stale from.
        let _ = self
            .first_attempt
            .compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed);
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
        // A feed that has never succeeded is measured from its first attempt, not treated as
        // healthy forever: "we have not managed to reach the endpoint yet" is the same outage as
        // "we stopped being able to", and is the more likely one at boot.
        let since = match self.last_success.load(Ordering::Relaxed) {
            0 => match self.first_attempt.load(Ordering::Relaxed) {
                0 => return true, // nothing attempted yet
                first => first,
            },
            last => last,
        };
        now.saturating_sub(since) <= self.silence_budget_secs
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
                .filter_map(
                    |record| match record.to_raw_order(chain, client.protocol()) {
                        Ok(raw) => Some(raw),
                        Err(e) => {
                            warn!("orders feed: unusable record {}: {}", record.order_hash, e);
                            None
                        }
                    },
                )
                .collect()
        }
        Err(OrdersApiError::Refused) => {
            health.record_failure(now_unix(), true);
            warn!("orders feed refused by the endpoint; the order stream is down");
            Vec::new()
        }
        Err(e) => {
            health.record_failure(now_unix(), false);
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
        health.record_failure(1_001, true);
        assert!(!health.is_live(1_001));
    }

    #[test]
    fn a_transient_failure_does_not_fail_readiness_on_its_own() {
        let health = FeedHealth::new(Duration::from_secs(60));
        health.record_success(1_000);
        health.record_failure(1_005, false);
        assert!(health.is_live(1_010));
        assert_eq!(health.consecutive_failures(), 1);
    }

    /// A feed whose very first poll never lands is down, not pending. Measuring from the first
    /// attempt is what separates "we have not reached the endpoint yet" from "nothing to report" —
    /// without it a broken feed reports healthy for the life of the process.
    #[test]
    fn a_feed_that_never_succeeds_goes_stale_from_its_first_attempt() {
        let health = FeedHealth::new(Duration::from_secs(60));
        assert!(health.is_live(1_000), "nothing attempted yet");
        health.record_failure(1_000, false);
        assert!(health.is_live(1_050), "inside the silence budget");
        assert!(
            !health.is_live(1_061),
            "past it, with no success ever recorded"
        );
    }

    #[test]
    fn a_success_clears_earlier_failures() {
        let health = FeedHealth::new(Duration::from_secs(60));
        health.record_failure(1_000, true);
        health.record_failure(1_001, false);
        health.record_success(2_000);
        assert_eq!(health.consecutive_failures(), 0);
        assert!(health.is_live(2_001));
    }
}
