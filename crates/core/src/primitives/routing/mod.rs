//! Routing value types — the solver's `RouteRequest` input, its `RoutePlan` output, and
//! the operator's `RoutingConfig`. The per-leg gas cost lives in [`gas`].

pub mod gas;
pub use gas::per_leg_cost;

use alloy_primitives::{Address, U256};

use crate::primitives::{IntentId, MakerId, StrategyHash};

/// Per-token metadata for the gas-cost conversion: the Binance spot symbol (which the price
/// feed subscribes to) and the token's decimals (base-unit scaling). The composition root
/// keys these by token address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenMeta {
    pub symbol: String,
    pub decimals: u8,
}

/// One intent's routing request — the pair, size, and direction to source. `exact_in`
/// fixes the taker's input (sell `amount` of `token_in`); otherwise it fixes the output
/// (deliver `amount` of `token_out`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteRequest {
    pub intent: IntentId,
    pub token_in: Address,
    pub token_out: Address,
    pub amount: U256,
    pub exact_in: bool,
}

/// One maker strategy's contribution to a route: `amount_in` of `token_in` in for
/// `amount_out` of `token_out` out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteLeg {
    pub maker: MakerId,
    pub strategy_hash: StrategyHash,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub amount_out: U256,
}

/// A sourcing plan for an intent — the legs to pull, and the resolver's spread. Execution
/// maps it to the filler's `SourceSwap[]`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RoutePlan {
    pub intent: IntentId,
    pub legs: Vec<RouteLeg>,
    pub expected_profit: U256,
    /// Price impact of the routed split, in percent.
    pub price_impact_pct: f64,
}

impl RoutePlan {
    pub fn new(
        intent: IntentId,
        legs: Vec<RouteLeg>,
        expected_profit: U256,
        price_impact_pct: f64,
    ) -> RoutePlan {
        RoutePlan {
            intent,
            legs,
            expected_profit,
            price_impact_pct,
        }
    }
}

/// Routing configuration. The gas **units** for one fill leg are a per-chain constant (one
/// Aqua `swap` — transferFrom + pull/push + the SwapVM program; measure with `forge test
/// --gas-report` on the filler). The per-leg *cost in the spread token's units* is derived
/// live from `gas_units_per_leg × gas_price ÷ token price` (see `deps/routing`) and handed to
/// the pure solver as a value.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct RoutingConfig {
    /// Funnel bound — the most candidates the solver ever optimizes over.
    pub max_candidates: usize,
    /// Split cap — the most legs a route may use (over-fragmentation guard); `0` behaves as `1`.
    pub max_legs: usize,
    /// Gas a single fill leg costs on this chain, in gas units.
    pub gas_units_per_leg: u64,
}

impl RoutingConfig {
    pub fn new(max_candidates: usize, max_legs: usize, gas_units_per_leg: u64) -> Self {
        Self {
            max_candidates,
            max_legs,
            gas_units_per_leg,
        }
    }
}
