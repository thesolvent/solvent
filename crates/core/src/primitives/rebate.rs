//! Values shared by rebate accrual, policy evaluation, and execution.

use std::collections::BTreeMap;

use alloy_primitives::{Address, Bytes, B256, U256};

use crate::primitives::execution::{ExecutionAuthorization, PolicySignature};
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
    /// Block containing the confirmed fill; policy waits until the registry has indexed it.
    pub confirmed_block: u64,
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
        confirmed_block: u64,
    ) -> Self {
        Self {
            trade_id,
            strategy,
            token_in,
            token_out,
            amount_in,
            amount_out,
            confirmed_block,
            allocation_weight,
        }
    }
}

/// One mined filler event proving a public rebate execution completed on chain.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebateExecutedEvent {
    pub batch_id: RebateBatchId,
    pub strategy_hash: crate::primitives::StrategyHash,
    pub executor: Address,
    pub maker: crate::primitives::MakerId,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub amount_out: U256,
    pub maker_rebate: U256,
    pub tx_hash: B256,
    pub block_number: u64,
    pub log_index: u64,
}

impl RebateExecutedEvent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        batch_id: RebateBatchId,
        strategy_hash: crate::primitives::StrategyHash,
        executor: Address,
        maker: crate::primitives::MakerId,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
        amount_out: U256,
        maker_rebate: U256,
        tx_hash: B256,
        block_number: u64,
        log_index: u64,
    ) -> Self {
        Self {
            batch_id,
            strategy_hash,
            executor,
            maker,
            token_in,
            token_out,
            amount_in,
            amount_out,
            maker_rebate,
            tx_hash,
            block_number,
            log_index,
        }
    }
}

/// Crash-recoverable record written before the rebate reservation is posted.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebateSettlement {
    pub plan: Box<RebatePlan>,
    pub reservation: ReservationId,
    pub event: RebateExecutedEvent,
    pub executed_at: u64,
}

impl RebateSettlement {
    pub fn new(
        plan: RebatePlan,
        reservation: ReservationId,
        event: RebateExecutedEvent,
        executed_at: u64,
    ) -> Self {
        Self {
            plan: Box::new(plan),
            reservation,
            event,
            executed_at,
        }
    }
}

/// Fresh executable prices for both sides of a token pair, expressed as output per input in base
/// units. The two sides are independent because a real order book has a spread.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebateMarket {
    pub token_a: Address,
    pub token_b: Address,
    pub a_to_b: Ratio,
    pub b_to_a: Ratio,
}

impl RebateMarket {
    pub fn new(token_a: Address, token_b: Address, a_to_b: Ratio, b_to_a: Ratio) -> Self {
        Self {
            token_a,
            token_b,
            a_to_b,
            b_to_a,
        }
    }

    pub fn rate(&self, token_in: Address, token_out: Address) -> Option<&Ratio> {
        match (token_in, token_out) {
            (input, output) if input == self.token_a && output == self.token_b => {
                Some(&self.a_to_b)
            }
            (input, output) if input == self.token_b && output == self.token_a => {
                Some(&self.b_to_a)
            }
            _ => None,
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

/// Token-denominated policy floors. The service adds a fresh gas estimate before assessment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebateMinimums {
    pub token: Address,
    pub minimum_maker_rebate: U256,
    pub minimum_executor_profit: U256,
}

impl RebateMinimums {
    pub fn new(token: Address, minimum_maker_rebate: U256, minimum_executor_profit: U256) -> Self {
        Self {
            token,
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

impl RebateAllocation {
    pub fn new(trade_id: TradeId, amount: U256) -> Self {
        Self { trade_id, amount }
    }
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

impl RebatePlan {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        batch_id: RebateBatchId,
        strategy: StrategyKey,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
        amount_out: U256,
        gross_surplus: U256,
        safe_gas_cost: U256,
        maker_rebate: U256,
        executor_profit: U256,
        deviation_bps: u64,
        allocations: Vec<RebateAllocation>,
    ) -> Self {
        Self {
            batch_id,
            strategy,
            token_in,
            token_out,
            amount_in,
            amount_out,
            gross_surplus,
            safe_gas_cost,
            maker_rebate,
            executor_profit,
            deviation_bps,
            allocations,
        }
    }
}

/// The signed, immutable transaction payload exposed to public executors.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebateExecution {
    pub plan: Box<RebatePlan>,
    pub reservation: ReservationId,
    pub authorization: ExecutionAuthorization,
    pub order: Bytes,
    pub policy_signature: PolicySignature,
    pub calldata: Bytes,
    pub published_at: u64,
}

impl RebateExecution {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plan: RebatePlan,
        reservation: ReservationId,
        authorization: ExecutionAuthorization,
        order: Bytes,
        policy_signature: PolicySignature,
        calldata: Bytes,
        published_at: u64,
    ) -> Self {
        Self {
            plan: Box::new(plan),
            reservation,
            authorization,
            order,
            policy_signature,
            calldata,
            published_at,
        }
    }
}

impl core::fmt::Debug for RebateExecution {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RebateExecution")
            .field("plan", &self.plan)
            .field("reservation", &self.reservation)
            .field("authorization", &self.authorization)
            .field("order", &"[REDACTED]")
            .field("policy_signature", &"[REDACTED]")
            .field("calldata", &"[REDACTED]")
            .field("published_at", &self.published_at)
            .finish()
    }
}

/// The durable batch state. Transitional states let restart recovery safely compensate an
/// interrupted reservation change before any authorization is exposed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RebateBatchState {
    Accumulating,
    Guarded,
    Preparing {
        plan: Box<RebatePlan>,
        reservation: ReservationId,
    },
    Ready(Box<RebateExecution>),
    Invalidating {
        reservation: ReservationId,
    },
}

/// One strategy's only open restoration batch and every mined trade leg that contributed to it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebateBatch {
    pub id: RebateBatchId,
    pub strategy: StrategyKey,
    pub pair: crate::primitives::registry::TokenPair,
    pub accruals: BTreeMap<TradeId, RebateAccrual>,
    pub state: RebateBatchState,
}

impl RebateBatch {
    pub fn new(
        id: RebateBatchId,
        pair: crate::primitives::registry::TokenPair,
        accrual: RebateAccrual,
    ) -> Self {
        Self {
            id,
            strategy: accrual.strategy,
            pair,
            accruals: BTreeMap::from([(accrual.trade_id, accrual)]),
            state: RebateBatchState::Accumulating,
        }
    }

    pub fn from_parts(
        id: RebateBatchId,
        strategy: StrategyKey,
        pair: crate::primitives::registry::TokenPair,
        accruals: BTreeMap<TradeId, RebateAccrual>,
        state: RebateBatchState,
    ) -> Self {
        Self {
            id,
            strategy,
            pair,
            accruals,
            state,
        }
    }
}

/// The policy result visible to the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RebateAssessment {
    Quoteable { deviation_bps: u64 },
    Guarded { deviation_bps: u64 },
    Ready(Box<RebatePlan>),
}
