//! An in-process order feed that mints and signs its own ERC-7683 orders — the E2E and demo source
//! until a hosted 7683 feed exists. Orders are built once up front; the feed then streams them.

use futures::stream::{BoxStream, StreamExt};

use solvent_core::deps::ingest::OrderFeed;
use solvent_core::primitives::ingest::RawOrder;

use super::builder::{OrderSpec, SignedOrderBuilder};

pub struct SelfHostedFeed {
    orders: Vec<RawOrder>,
}

impl SelfHostedFeed {
    /// Build and sign each spec once, ready to stream.
    pub fn new(
        builder: &SignedOrderBuilder,
        specs: &[OrderSpec],
        observed_at: u64,
    ) -> SelfHostedFeed {
        SelfHostedFeed {
            orders: specs
                .iter()
                .map(|s| builder.build(s, observed_at))
                .collect(),
        }
    }
}

impl OrderFeed for SelfHostedFeed {
    fn stream(&self) -> BoxStream<'static, RawOrder> {
        futures::stream::iter(self.orders.clone()).boxed()
    }
}
