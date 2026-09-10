//! Values shared by rebate accrual, policy evaluation, and execution.

use alloy_primitives::{Address, U256};

use crate::primitives::pricing::Ratio;
use crate::primitives::registry::StrategyKey;
use crate::primitives::trade::TradeId;
use crate::primitives::{RebateBatchId, ReservationId};

/// One mined trade leg that contributed to a strategy's price displacement.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebateAccrual {
    pub trade_id: TradeId,
    pub strategy: StrategyKey,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub amount_out: U256,
    /// Canonical-token weight for this trade's proportional share of the maker rebate.
    pub allocation_weight: U256,
}

impl RebateAccrual {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        trade_id: TradeId,
        strategy: StrategyKey,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
        amount_out: U256,
        allocation_weight: U256,
    ) -> Self {
        Self {
            trade_id,
            strategy,
            token_in,
            token_out,
            amount_in,
            amount_out,
            allocation_weight,
        }
    }
}

/// Fresh external price for one oriented token pair, expressed as output per input in base units.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebateMarket {
    pub token_in: Address,
    pub token_out: Address,
    pub output_per_input: Ratio,
}

impl RebateMarket {
    pub fn new(token_in: Address, token_out: Address, output_per_input: Ratio) -> Self {
        Self {
            token_in,
            token_out,
            output_per_input,
        }
    }
}

/// Gas and minimum-profit requirements denominated in one possible restoration input token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebateRequirements {
    pub token: Address,
    pub gas_cost: U256,
    pub minimum_maker_rebate: U256,
    pub minimum_executor_profit: U256,
}

impl RebateRequirements {
    pub fn new(
        token: Address,
        gas_cost: U256,
        minimum_maker_rebate: U256,
        minimum_executor_profit: U256,
    ) -> Self {
        Self {
            token,
            gas_cost,
            minimum_maker_rebate,
            minimum_executor_profit,
        }
    }
}

/// One affected trade's share of the maker rebate.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebateAllocation {
    pub trade_id: TradeId,
    pub amount: U256,
}

/// An exact-output restoration that clears all policy gates.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebatePlan {
    pub batch_id: RebateBatchId,
    pub strategy: StrategyKey,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub amount_out: U256,
    pub gross_surplus: U256,
    pub safe_gas_cost: U256,
    pub maker_rebate: U256,
    pub executor_profit: U256,
    pub deviation_bps: u64,
    pub allocations: Vec<RebateAllocation>,
}

/// The batch state; a ready plan carries the inventory reservation that makes it firm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RebateBatchState {
    Accumulating,
    Guarded,
    Ready {
        plan: Box<RebatePlan>,
        reservation: ReservationId,
    },
}

/// The policy result visible to the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RebateAssessment {
    Quoteable { deviation_bps: u64 },
    Guarded { deviation_bps: u64 },
    Ready(Box<RebatePlan>),
}
