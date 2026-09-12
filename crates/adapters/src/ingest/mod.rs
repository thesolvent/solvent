//! Ingest adapters: per-protocol normalizers (and, later, order feeds).

mod aqua;
pub mod erc7683;
pub mod fill;
pub mod uniswapx;

pub use fill::ProtocolFillBuilder;
