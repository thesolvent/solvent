//! Inbound API handlers — thin translators from a request to a core call to a response envelope.
//! Business logic lives in core services, never here.

pub mod activity;
pub mod assets;
pub mod balances;
pub mod config;
pub mod makers;
pub mod pairs;
pub mod pools;
pub mod positions;
pub mod rebates;
pub mod stats;
pub mod swap;
pub mod trades;
pub mod uniswapx_feed;
