//! The pool slice: the read-surface that aggregates active strategies into pool views, plus the
//! depth curve (executable liquidity per trade size).

mod depth;
mod service;

pub use crate::primitives::pool::{
    CurveMix, DepthPoint, Pool, PoolDepth, PoolDetail, PoolMaker, PoolType, Side,
};
pub use depth::DepthService;
pub use service::PoolService;
