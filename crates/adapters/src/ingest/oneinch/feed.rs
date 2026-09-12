//! The live 1inch order feed: polls the Orderbook API and streams what it finds.
//!
//! Mirrors UniswapX's `HostedFeed` (same polling shape, same shared [`FeedHealth`] the readiness
//! endpoint reads), but there is exactly one scope — the whole book — since this codebase has no
//! exclusive-filler concept on 1inch orders to poll separately.
//!
//! Duplicates are expected, not exceptional, same as the UniswapX feed: each poll returns the
//! current book, so an order appears on every tick until it is filled, cancelled, or expires. The
//! ingest pipeline's own dedup absorbs that.

use std::sync::Arc;
use std::time::Duration;

use futures::stream::{BoxStream, StreamExt};

use solvent_core::deps::ingest::OrderFeed;
use solvent_core::primitives::ingest::RawOrder;
use solvent_core::primitives::ChainId;

use solvent_core::obs::{debug, warn};

use crate::ingest::uniswapx::FeedHealth;

use super::orders_api::{OneInchApiClient, OneInchApiError};

/// Polls the Orderbook API forever, yielding every open order it sees.
pub struct OneInchFeed {
    client: Arc<OneInchApiClient>,
    chain: ChainId,
    interval: Duration,
    health: Arc<FeedHealth>,
}

impl OneInchFeed {
    pub fn new(
        client: Arc<OneInchApiClient>,
        chain: ChainId,
        interval: Duration,
        health: Arc<FeedHealth>,
    ) -> OneInchFeed {
        OneInchFeed {
            client,
            chain,
            interval,
            health,
        }
    }

    pub fn health(&self) -> Arc<FeedHealth> {
        Arc::clone(&self.health)
    }
}

impl OrderFeed for OneInchFeed {
    fn stream(&self) -> BoxStream<'static, RawOrder> {
        let client = Arc::clone(&self.client);
        let health = Arc::clone(&self.health);
        let chain = self.chain;
        let interval = self.interval;

        let ticks = futures::stream::unfold((), move |()| {
            let client = Arc::clone(&client);
            let health = Arc::clone(&health);
            async move {
                tokio::time::sleep(interval).await;
                let orders = poll_once(&client, &health, chain).await;
                Some((futures::stream::iter(orders), ()))
            }
        });
        ticks.flatten().boxed()
    }
}

/// One request. Never fails the stream: a poll that errors is logged, recorded against health, and
/// retried on the next tick, same as the UniswapX feed.
async fn poll_once(
    client: &OneInchApiClient,
    health: &FeedHealth,
    chain: ChainId,
) -> Vec<RawOrder> {
    let now = now_unix();
    match client.open_orders().await {
        Ok(records) => {
            health.record_success(now);
            records
                .iter()
                .filter(|record| record.order_invalid_reason.is_none())
                .filter_map(|record| match record.to_raw_order(chain, now) {
                    Ok(raw) => Some(raw),
                    Err(e) => {
                        warn!("1inch feed: unusable record {}: {}", record.order_hash, e);
                        None
                    }
                })
                .collect()
        }
        Err(OneInchApiError::Unauthorized) => {
            health.record_failure(now, true);
            warn!("1inch feed refused by the endpoint (bad or missing api key); the order stream is down");
            Vec::new()
        }
        Err(e) => {
            health.record_failure(now, false);
            debug!("1inch feed poll failed: {}", e);
            Vec::new()
        }
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
