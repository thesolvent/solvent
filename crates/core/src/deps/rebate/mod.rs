//! Durable state, executable market data, and calldata construction used by rebate orchestration.

pub mod call_builder;
pub mod market_book;
pub mod store;

pub use call_builder::{RebateCallBuilder, RebateCallBuilderError};
pub use market_book::{RebateMarketBook, RebateMarketBookError, RebateMarketRequest};
pub use store::{RebateStore, RebateStoreError};
