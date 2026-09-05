//! The pool slice: the read-surface that aggregates active strategies into pool views.

mod service;

pub use crate::primitives::pool::{Pool, PoolDetail, PoolMaker, PoolType};
pub use service::PoolService;
