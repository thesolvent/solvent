//! The order-feed port: a source of raw orders. Each protocol/source is an adapter behind this
//! trait — hosted poll, self-hosted, or replay — and the pipeline fans several in at once.

use futures::stream::BoxStream;

use crate::primitives::ingest::RawOrder;

/// A source of raw orders as a never-ending stream. The adapter owns its own acquisition (polling,
/// backfill, retry) and yields already-clean `RawOrder`s; a fatal source condition ends the stream.
pub trait OrderFeed: Send + Sync {
    fn stream(&self) -> BoxStream<'static, RawOrder>;
}
