//! The router's output: how an intent's output is sourced across maker strategies.

use alloy_primitives::{Address, U256};

use crate::primitives::{IntentId, MakerId, StrategyHash};

/// One maker strategy's contribution to a route: `amount_in` of `token_in` in for
/// `amount_out` of `token_out` out, pulled from `strategy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteLeg {
    pub maker: MakerId,
    pub strategy_hash: StrategyHash,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub amount_out: U256,
}

/// A sourcing plan for an intent — the legs to pull, and the resolver's spread. A
/// routing value type; execution maps it to the filler's `SourceSwap[]`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RoutePlan {
    pub intent: IntentId,
    pub legs: Vec<RouteLeg>,
    pub expected_profit: U256,
}
