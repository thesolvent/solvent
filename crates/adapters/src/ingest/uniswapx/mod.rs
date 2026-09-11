//! UniswapX V2 ingest adapters.

mod builder;
mod codec;
mod cosigner;
mod feed;
mod fill;
mod hosted;
mod normalizer;
mod orders_api;
mod v1_normalizer;

pub use builder::{OrderSpec, SignedOrderBuilder};
pub use cosigner::ServerCosigner;
pub use feed::SelfHostedFeed;
pub use fill::UniswapXFillBuilder;
pub use hosted::{FeedHealth, HostedFeed};
pub use normalizer::UniswapXV2Normalizer;
pub use orders_api::{OrderRecord, OrdersApiClient, OrdersApiError, Scope};
pub use v1_normalizer::UniswapXV1Normalizer;
