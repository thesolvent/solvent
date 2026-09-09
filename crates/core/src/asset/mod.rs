//! The asset slice: one authority that answers everything about an asset.

mod history;
mod service;

pub use crate::primitives::asset::{
    Asset, PairInfo, PairKind, PairPriceHistory, PairPricePoint, PairWallet, PriceHistoryPeriod,
    Token, TokenList, TokenMeta,
};
pub use history::PairHistoryService;
pub use service::AssetManager;
