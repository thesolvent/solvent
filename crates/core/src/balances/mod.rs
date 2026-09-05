//! The balances slice: the connected wallet's per-token holdings, served from the balances oracle.

mod service;

pub use crate::primitives::balances::{Holdings, TokenBalance};
pub use service::BalancesService;
