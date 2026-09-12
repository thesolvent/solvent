//! Ingest adapters: per-protocol normalizers and order feeds.

mod aqua;
pub mod erc7683;
pub mod fill;
pub mod oneinch;
pub mod uniswapx;

pub use fill::ProtocolFillBuilder;
