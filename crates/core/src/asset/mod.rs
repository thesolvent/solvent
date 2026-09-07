//! The asset slice: one authority that answers everything about an asset.

mod service;

pub use crate::primitives::asset::{
    Asset, PairInfo, PairKind, PairWallet, Token, TokenList, TokenMeta,
};
pub use service::AssetManager;
