//! One-live-batch orchestration around the pure rebate policy.

use std::collections::BTreeMap;
use std::sync::Arc;

use alloy_primitives::{keccak256, FixedBytes};
use alloy_sol_types::SolValue;
use tokio::sync::Mutex;

use crate::ledger::{AvailableSnapshot, LedgerService};
use crate::primitives::ledger::{LedgerError, ReservationSource};
use crate::primitives::rebate::{
    RebateAccrual, RebateAssessment, RebateBatchState, RebateMarket, RebateRequirements,
};
use crate::primitives::registry::{MakerStrategy, StrategyKey, TokenPair};
use crate::primitives::{RebateBatchId, ReservationId, SolventError};
use crate::routing::StrategyGuard;

use super::{RebateError, RebatePolicy};

pub struct RebateEvaluationInput<'a> {
    pub strategy: &'a MakerStrategy,
    pub caps: &'a AvailableSnapshot,
    pub market: &'a RebateMarket,
    pub requirements: &'a [RebateRequirements],
    pub reservation_id: ReservationId,
    pub reservation_ttl_secs: u64,
}

pub struct RebateService {
    batches: Mutex<BTreeMap<StrategyKey, RebateBatch>>,
    policy: RebatePolicy,
    ledger: Arc<LedgerService>,
    guards: Arc<StrategyGuard>,
}

impl RebateService {
    pub fn new(
        policy: RebatePolicy,
        ledger: Arc<LedgerService>,
        guards: Arc<StrategyGuard>,
    ) -> Self {
        Self {
            batches: Mutex::new(BTreeMap::new()),
            policy,
            ledger,
            guards,
        }
    }

    /// Idempotently append one mined leg to the strategy's only live batch.
    pub async fn accrue(&self, accrual: RebateAccrual) -> Result<RebateBatchId, SolventError> {
        validate_accrual(&accrual)?;
        // Serializing transitions keeps a strategy from acquiring two concurrent live batches.
        let mut batches = self.batches.lock().await;
        if let Some(batch) = batches.get_mut(&accrual.strategy) {
            if let Some(existing) = batch.accruals.get(&accrual.trade_id) {
                return match existing == &accrual {
                    true => Ok(batch.id),
                    false => Err(RebateError::InvalidAccrual(
                        "trade and strategy identify conflicting legs",
                    )
                    .into()),
                };
            }
            if batch.pair != TokenPair::new(accrual.token_in, accrual.token_out) {
                return Err(RebateError::InvalidAccrual("strategy pair changed").into());
            }
            if let RebateBatchState::Ready { reservation, .. } = batch.state {
                // Ready is still private here, so changed economics require a fresh inventory hold.
                self.guards.guard(accrual.strategy);
                self.ledger.void(reservation).await?;
                batch.state = RebateBatchState::Guarded;
            }
            batch.accruals.insert(accrual.trade_id, accrual);
            return Ok(batch.id);
        }

        let id = batch_id(&accrual);
        let strategy = accrual.strategy;
        let pair = TokenPair::new(accrual.token_in, accrual.token_out);
        batches.insert(strategy, RebateBatch::new(id, pair, accrual));
        Ok(id)
    }

    /// Re-evaluate the live batch, publishing the strategy guard before reserving a firm plan.
    pub async fn evaluate(
        &self,
        input: RebateEvaluationInput<'_>,
    ) -> Result<RebateAssessment, SolventError> {
        let mut batches = self.batches.lock().await;
        let batch = batches
            .get_mut(&input.strategy.key)
            .ok_or(RebateError::StrategyMismatch(
                input.strategy.key.strategy_hash,
            ))?;
        if let RebateBatchState::Ready { plan, .. } = &batch.state {
            return Ok(RebateAssessment::Ready(plan.clone()));
        }
        let accruals: Vec<_> = batch.accruals.values().cloned().collect();
        let assessment = self.policy.assess(
            batch.id,
            &accruals,
            input.strategy,
            input.caps,
            input.market,
            input.requirements,
        )?;

        match assessment {
            RebateAssessment::Quoteable { deviation_bps } => {
                batch.state = RebateBatchState::Accumulating;
                self.guards.unguard(&input.strategy.key);
                Ok(RebateAssessment::Quoteable { deviation_bps })
            }
            RebateAssessment::Guarded { deviation_bps } => {
                batch.state = RebateBatchState::Guarded;
                self.guards.guard(input.strategy.key);
                Ok(RebateAssessment::Guarded { deviation_bps })
            }
            RebateAssessment::Ready(plan) => {
                // Publish the guard before promising the same strategy inventory to a rebate.
                self.guards.guard(input.strategy.key);
                let sources = vec![ReservationSource {
                    maker: input.strategy.key.maker,
                    strategy_hash: input.strategy.key.strategy_hash,
                    token: plan.token_out,
                    amount: plan.amount_out,
                }];
                match self
                    .ledger
                    .reserve_rebate(
                        input.reservation_id,
                        batch.id,
                        sources,
                        input.reservation_ttl_secs,
                    )
                    .await
                {
                    Ok(()) => {
                        batch.state = RebateBatchState::Ready {
                            plan: plan.clone(),
                            reservation: input.reservation_id,
                        };
                        Ok(RebateAssessment::Ready(plan))
                    }
                    Err(SolventError::Ledger(LedgerError::Insufficient(_))) => {
                        // Capacity contention must fail closed until fresh balances are evaluated.
                        batch.state = RebateBatchState::Guarded;
                        Ok(RebateAssessment::Guarded {
                            deviation_bps: plan.deviation_bps,
                        })
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }

    /// Release a live batch and any firm reservation, then restore routing visibility.
    pub async fn discard(&self, strategy: &StrategyKey) -> Result<bool, SolventError> {
        let mut batches = self.batches.lock().await;
        let Some(batch) = batches.get(strategy) else {
            return Ok(false);
        };
        if let RebateBatchState::Ready { reservation, .. } = batch.state {
            self.ledger.void(reservation).await?;
        }
        batches.remove(strategy);
        self.guards.unguard(strategy);
        Ok(true)
    }
}

#[derive(Debug, Clone)]
struct RebateBatch {
    id: RebateBatchId,
    pair: TokenPair,
    accruals: BTreeMap<crate::primitives::trade::TradeId, RebateAccrual>,
    state: RebateBatchState,
}

impl RebateBatch {
    fn new(id: RebateBatchId, pair: TokenPair, accrual: RebateAccrual) -> Self {
        Self {
            id,
            pair,
            accruals: BTreeMap::from([(accrual.trade_id, accrual)]),
            state: RebateBatchState::Accumulating,
        }
    }
}

fn validate_accrual(accrual: &RebateAccrual) -> Result<(), RebateError> {
    if accrual.token_in == accrual.token_out {
        return Err(RebateError::InvalidAccrual("tokens are identical"));
    }
    if accrual.amount_in.is_zero()
        || accrual.amount_out.is_zero()
        || accrual.allocation_weight.is_zero()
    {
        return Err(RebateError::InvalidAccrual(
            "amounts and allocation weight must be non-zero",
        ));
    }
    Ok(())
}

fn batch_id(accrual: &RebateAccrual) -> RebateBatchId {
    let trade = FixedBytes::<16>::from(accrual.trade_id.0.to_bytes());
    // Stable batch identity makes replaying the first accrual idempotent after persistence.
    RebateBatchId(keccak256(
        (
            accrual.strategy.maker.0,
            accrual.strategy.app,
            accrual.strategy.strategy_hash.0,
            trade,
        )
            .abi_encode(),
    ))
}
