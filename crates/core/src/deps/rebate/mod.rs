//! Durable state, executable market data, and calldata construction used by rebate orchestration.

pub mod call_builder;
pub mod chain_source;
pub mod market_book;
pub mod store;
pub mod trade_source;

pub use call_builder::{RebateCallBuilder, RebateCallBuilderError};
pub use chain_source::{RebateChainSource, RebateChainSourceError};
pub use market_book::{RebateMarketBook, RebateMarketBookError, RebateMarketRequest};
pub use store::{RebateStore, RebateStoreError};
pub use trade_source::{RebateAccrualSource, RebateAccrualSourceError};
