//! One-live-batch orchestration around the pure rebate policy.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use alloy_primitives::{keccak256, FixedBytes, U256};
use alloy_sol_types::SolValue;
use tokio::sync::Mutex;

use crate::asset::AssetManager;
use crate::deps::rebate::{RebateCallBuilder, RebateMarketBook, RebateMarketRequest, RebateStore};
use crate::deps::routing::GasPrice;
use crate::ledger::{AvailableSnapshot, LedgerService};
use crate::primitives::execution::ExecutionKind;
use crate::primitives::ledger::{LedgerError, ReservationSource};
use crate::primitives::pricing::Ratio;
use crate::primitives::rebate::{
    RebateAccrual, RebateAssessment, RebateBatch, RebateBatchState, RebateExecution,
    RebateMinimums, RebateRequirements,
};
use crate::primitives::registry::{MakerStrategy, StrategyKey, TokenPair};
use crate::primitives::{RebateBatchId, ReservationId, SolventError};
use crate::routing::StrategyGuard;

use super::{RebateError, RebatePolicy};

pub struct RebateEvaluationInput<'a> {
    pub strategy: &'a MakerStrategy,
    pub caps: &'a AvailableSnapshot,
    pub minimums: &'a [RebateMinimums],
    pub reservation_id: ReservationId,
    pub reservation_ttl_secs: u64,
    pub deadline_block: u64,
    pub published_at: u64,
}

#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct RebateServiceConfig {
    pub native_token: alloy_primitives::Address,
    pub gas_units: u64,
    pub market_max_age_secs: u64,
}

pub struct RebateMarketData {
    book: Arc<dyn RebateMarketBook>,
    gas: Arc<dyn GasPrice>,
    assets: Arc<AssetManager>,
}

impl RebateMarketData {
    pub fn new(
        book: Arc<dyn RebateMarketBook>,
        gas: Arc<dyn GasPrice>,
        assets: Arc<AssetManager>,
    ) -> Self {
        Self { book, gas, assets }
    }
}

impl RebateServiceConfig {
    pub fn new(
        native_token: alloy_primitives::Address,
        gas_units: u64,
        market_max_age_secs: u64,
    ) -> Self {
        Self {
            native_token,
            gas_units,
            market_max_age_secs,
        }
    }
}

pub struct RebateService {
    batches: Mutex<BTreeMap<StrategyKey, RebateBatch>>,
    policy: RebatePolicy,
    ledger: Arc<LedgerService>,
    guards: Arc<StrategyGuard>,
    store: Arc<dyn RebateStore>,
    call_builder: Arc<dyn RebateCallBuilder>,
    market: RebateMarketData,
    config: RebateServiceConfig,
}

impl RebateService {
    pub fn new(
        policy: RebatePolicy,
        ledger: Arc<LedgerService>,
        guards: Arc<StrategyGuard>,
        store: Arc<dyn RebateStore>,
        call_builder: Arc<dyn RebateCallBuilder>,
        market: RebateMarketData,
        config: RebateServiceConfig,
    ) -> Self {
        Self {
            batches: Mutex::new(BTreeMap::new()),
            policy,
            ledger,
            guards,
            store,
            call_builder,
            market,
            config,
        }
    }

    /// Rebuild open batches and routing guards after the ledger has recovered its durable holds.
    pub async fn recover(&self) -> Result<(), SolventError> {
        let mut recovered = BTreeMap::new();
        for mut batch in self.store.open_batches().await? {
            validate_batch(&batch)?;
            if recovered.contains_key(&batch.strategy) {
                return Err(RebateError::InvalidAccrual(
                    "multiple open batches target one strategy",
                )
                .into());
            }

            match &batch.state {
                RebateBatchState::Accumulating => self.guards.unguard(&batch.strategy),
                RebateBatchState::Guarded => self.guards.guard(batch.strategy).await,
                RebateBatchState::Ready(execution) => {
                    self.guards.guard(batch.strategy).await;
                    if self
                        .ledger
                        .reservation_sources(execution.reservation)
                        .await
                        .is_none()
                    {
                        batch.state = RebateBatchState::Guarded;
                        self.store.save(&batch).await?;
                    }
                }
                RebateBatchState::Preparing { reservation, .. }
                | RebateBatchState::Invalidating { reservation } => {
                    self.guards.guard(batch.strategy).await;
                    self.release_if_held(*reservation).await?;
                    batch.state = RebateBatchState::Guarded;
                    self.store.save(&batch).await?;
                }
            }
            recovered.insert(batch.strategy, batch);
        }
        *self.batches.lock().await = recovered;
        Ok(())
    }

    /// Idempotently append one mined leg to the strategy's only live batch.
    pub async fn accrue(&self, accrual: RebateAccrual) -> Result<RebateBatchId, SolventError> {
        validate_accrual(&accrual)?;
        // Serializing transitions keeps a strategy from acquiring two concurrent live batches.
        let mut batches = self.batches.lock().await;
        if let Some(batch) = batches.get_mut(&accrual.strategy) {
            if let Some(existing) = batch.accruals.get(&accrual.trade_id) {
                if existing != &accrual {
                    return Err(RebateError::InvalidAccrual(
                        "trade and strategy identify conflicting legs",
                    )
                    .into());
                }
                self.finish_interrupted_transition(batch).await?;
                return Ok(batch.id);
            }
            if batch.pair != TokenPair::new(accrual.token_in, accrual.token_out) {
                return Err(RebateError::InvalidAccrual("strategy pair changed").into());
            }
            let was_accumulating = matches!(batch.state, RebateBatchState::Accumulating);
            self.guards.guard(accrual.strategy).await;
            let reservation = active_reservation(&batch.state);
            let mut next = batch.clone();
            next.accruals.insert(accrual.trade_id, accrual);
            match reservation {
                Some(reservation) => {
                    // Persist the compensation marker before releasing a previously promised hold.
                    next.state = RebateBatchState::Invalidating { reservation };
                    self.store.save(&next).await?;
                    *batch = next.clone();
                    self.release_if_held(reservation).await?;
                    next.state = RebateBatchState::Guarded;
                }
                None => next.state = RebateBatchState::Guarded,
            }
            if let Err(error) = self.store.save(&next).await {
                if reservation.is_none() && was_accumulating {
                    self.guards.unguard(&batch.strategy);
                }
                return Err(error.into());
            }
            *batch = next;
            return Ok(batch.id);
        }

        let id = batch_id(&accrual);
        let strategy = accrual.strategy;
        let pair = TokenPair::new(accrual.token_in, accrual.token_out);
        self.guards.guard(strategy).await;
        let mut batch = RebateBatch::new(id, pair, accrual);
        // A fresh external book must explicitly prove the displaced strategy safe before restart
        // recovery may return it to routing.
        batch.state = RebateBatchState::Guarded;
        if let Err(error) = self.store.save(&batch).await {
            self.guards.unguard(&strategy);
            return Err(error.into());
        }
        batches.insert(strategy, batch);
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
        self.finish_interrupted_transition(batch).await?;
        if let RebateBatchState::Ready(execution) = &batch.state {
            return Ok(RebateAssessment::Ready(execution.plan.clone()));
        }
        let accruals: Vec<_> = batch.accruals.values().cloned().collect();
        let (market, requirements) = self.economics(input.strategy, input.minimums).await?;
        let assessment = self.policy.assess(
            batch.id,
            &accruals,
            input.strategy,
            input.caps,
            &market,
            &requirements,
        )?;

        match assessment {
            RebateAssessment::Quoteable { deviation_bps } => {
                let mut next = batch.clone();
                next.state = RebateBatchState::Accumulating;
                self.store.save(&next).await?;
                *batch = next;
                self.guards.unguard(&input.strategy.key);
                Ok(RebateAssessment::Quoteable { deviation_bps })
            }
            RebateAssessment::Guarded { deviation_bps } => {
                self.guards.guard(input.strategy.key).await;
                let mut next = batch.clone();
                next.state = RebateBatchState::Guarded;
                self.store.save(&next).await?;
                *batch = next;
                Ok(RebateAssessment::Guarded { deviation_bps })
            }
            RebateAssessment::Ready(plan) => {
                // Publish the guard before promising the same strategy inventory to a rebate.
                self.guards.guard(input.strategy.key).await;
                let sources = vec![ReservationSource {
                    maker: input.strategy.key.maker,
                    strategy_hash: input.strategy.key.strategy_hash,
                    token: plan.token_out,
                    amount: plan.amount_out,
                }];
                let mut next = batch.clone();
                next.state = RebateBatchState::Preparing {
                    plan: plan.clone(),
                    reservation: input.reservation_id,
                };
                self.store.save(&next).await?;
                *batch = next;
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
                        let execution = match self
                            .call_builder
                            .build(
                                input.strategy,
                                *plan.clone(),
                                input.reservation_id,
                                input.deadline_block,
                                input.published_at,
                            )
                            .await
                        {
                            Ok(execution) => execution,
                            Err(error) => {
                                self.release_if_held(input.reservation_id).await?;
                                batch.state = RebateBatchState::Guarded;
                                self.store.save(batch).await?;
                                return Err(error.into());
                            }
                        };
                        let mut ready = batch.clone();
                        ready.state = RebateBatchState::Ready(Box::new(execution));
                        if let Err(error) = self.store.save(&ready).await {
                            self.release_if_held(input.reservation_id).await?;
                            return Err(error.into());
                        }
                        *batch = ready;
                        Ok(RebateAssessment::Ready(plan))
                    }
                    Err(SolventError::Ledger(LedgerError::Insufficient(_))) => {
                        // Capacity contention must fail closed until fresh balances are evaluated.
                        batch.state = RebateBatchState::Guarded;
                        self.store.save(batch).await?;
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
        let Some(batch) = batches.get_mut(strategy) else {
            return Ok(false);
        };
        if let Some(reservation) = active_reservation(&batch.state) {
            batch.state = RebateBatchState::Invalidating { reservation };
            self.store.save(batch).await?;
            self.release_if_held(reservation).await?;
        }
        self.store.close(batch.id).await?;
        batches.remove(strategy);
        self.guards.unguard(strategy);
        Ok(true)
    }

    /// Ready work is cloned from the immutable persisted payload; reads never mint a new nonce or
    /// signature.
    pub async fn ready(&self) -> Vec<RebateExecution> {
        self.batches
            .lock()
            .await
            .values()
            .filter_map(|batch| match &batch.state {
                RebateBatchState::Ready(execution) => Some((**execution).clone()),
                _ => None,
            })
            .collect()
    }

    pub async fn ready_by_id(&self, id: RebateBatchId) -> Option<RebateExecution> {
        self.batches
            .lock()
            .await
            .values()
            .find_map(|batch| match &batch.state {
                RebateBatchState::Ready(execution) if batch.id == id => Some((**execution).clone()),
                _ => None,
            })
    }

    async fn finish_interrupted_transition(
        &self,
        batch: &mut RebateBatch,
    ) -> Result<(), SolventError> {
        let reservation = match &batch.state {
            RebateBatchState::Preparing { reservation, .. }
            | RebateBatchState::Invalidating { reservation } => Some(*reservation),
            _ => None,
        };
        if let Some(reservation) = reservation {
            self.guards.guard(batch.strategy).await;
            self.release_if_held(reservation).await?;
            batch.state = RebateBatchState::Guarded;
            self.store.save(batch).await?;
        }
        Ok(())
    }

    async fn release_if_held(&self, reservation: ReservationId) -> Result<(), SolventError> {
        if self.ledger.reservation_sources(reservation).await.is_some() {
            self.ledger.void(reservation).await?;
        }
        Ok(())
    }

    async fn economics(
        &self,
        strategy: &MakerStrategy,
        minimums: &[RebateMinimums],
    ) -> Result<
        (
            crate::primitives::rebate::RebateMarket,
            Vec<RebateRequirements>,
        ),
        SolventError,
    > {
        let pair = strategy.pair().ok_or(RebateError::MarketPairMismatch)?;
        let market = self
            .market
            .book
            .market(RebateMarketRequest::new(
                pair.lo,
                self.market.assets.decimals(&pair.lo),
                pair.hi,
                self.market.assets.decimals(&pair.hi),
                self.config.market_max_age_secs,
            ))
            .await?;
        let gas_native = U256::from(self.market.gas.gas_price_wei().await?)
            .checked_mul(U256::from(self.config.gas_units))
            .ok_or(RebateError::Arithmetic)?;
        let mut requirements = Vec::with_capacity(minimums.len());
        for minimum in minimums {
            if minimum.token != pair.lo && minimum.token != pair.hi {
                return Err(RebateError::MissingRequirements(minimum.token).into());
            }
            let gas_cost = self.gas_cost_in(gas_native, minimum.token).await?;
            requirements.push(RebateRequirements::new(
                minimum.token,
                gas_cost,
                minimum.minimum_maker_rebate,
                minimum.minimum_executor_profit,
            ));
        }
        Ok((market, requirements))
    }

    async fn gas_cost_in(
        &self,
        gas_native: U256,
        token: alloy_primitives::Address,
    ) -> Result<U256, SolventError> {
        if token == self.config.native_token {
            return Ok(gas_native);
        }
        let conversion = self
            .market
            .book
            .market(RebateMarketRequest::new(
                self.config.native_token,
                self.market.assets.decimals(&self.config.native_token),
                token,
                self.market.assets.decimals(&token),
                self.config.market_max_age_secs,
            ))
            .await?;
        let native_per_token = conversion
            .rate(token, self.config.native_token)
            .cloned()
            .ok_or(RebateError::MarketPairMismatch)?;
        (Ratio::from(gas_native)
            * native_per_token
                .invert()
                .ok_or(RebateError::MarketPairMismatch)?)
        .ceil()
        .ok_or_else(|| RebateError::Arithmetic.into())
    }
}

fn active_reservation(state: &RebateBatchState) -> Option<ReservationId> {
    match state {
        RebateBatchState::Preparing { reservation, .. }
        | RebateBatchState::Invalidating { reservation } => Some(*reservation),
        RebateBatchState::Ready(execution) => Some(execution.reservation),
        RebateBatchState::Accumulating | RebateBatchState::Guarded => None,
    }
}

fn validate_batch(batch: &RebateBatch) -> Result<(), RebateError> {
    if batch.accruals.is_empty()
        || batch.accruals.iter().any(|(trade, accrual)| {
            *trade != accrual.trade_id
                || accrual.strategy != batch.strategy
                || TokenPair::new(accrual.token_in, accrual.token_out) != batch.pair
                || validate_accrual(accrual).is_err()
        })
    {
        return Err(RebateError::InvalidAccrual(
            "persisted batch contains invalid accruals",
        ));
    }
    let plan = match &batch.state {
        RebateBatchState::Preparing { plan, .. } => Some(plan.as_ref()),
        RebateBatchState::Ready(execution) => Some(execution.plan.as_ref()),
        _ => None,
    };
    if plan.is_some_and(|plan| !plan_matches_batch(plan, batch)) {
        return Err(RebateError::InvalidAccrual(
            "persisted plan does not match its batch",
        ));
    }
    if let RebateBatchState::Ready(execution) = &batch.state {
        let authorization = &execution.authorization;
        let plan = execution.plan.as_ref();
        if authorization.kind != ExecutionKind::Rebate
            || authorization.context_hash != batch.id.0
            || authorization.strategy_hash != batch.strategy.strategy_hash
            || authorization.maker != batch.strategy.maker
            || authorization.token_in != plan.token_in
            || authorization.token_out != plan.token_out
            || authorization.amount_out != plan.amount_out
            || authorization.amount_in_limit != plan.amount_in
            || authorization.rebate_amount != plan.maker_rebate
            || authorization.deadline_block == 0
            || execution.order.is_empty()
            || execution.policy_signature.as_bytes().len() != 65
            || execution.calldata.is_empty()
        {
            return Err(RebateError::InvalidAccrual(
                "persisted execution does not match its plan",
            ));
        }
    }
    Ok(())
}

fn plan_matches_batch(plan: &crate::primitives::rebate::RebatePlan, batch: &RebateBatch) -> bool {
    if plan.batch_id != batch.id
        || plan.strategy != batch.strategy
        || TokenPair::new(plan.token_in, plan.token_out) != batch.pair
        || plan.amount_in.is_zero()
        || plan.amount_out.is_zero()
        || plan.maker_rebate.is_zero()
    {
        return false;
    }
    let allocation_trades: BTreeSet<_> = plan
        .allocations
        .iter()
        .map(|allocation| allocation.trade_id)
        .collect();
    let allocations_match = allocation_trades.len() == plan.allocations.len()
        && allocation_trades.len() == batch.accruals.len()
        && allocation_trades
            .iter()
            .all(|trade_id| batch.accruals.contains_key(trade_id));
    let total = plan
        .allocations
        .iter()
        .try_fold(U256::ZERO, |sum, allocation| {
            sum.checked_add(allocation.amount)
        });
    allocations_match && total == Some(plan.maker_rebate)
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

#[cfg(test)]
mod tests {
    use alloy_primitives::{Address, B256, U256};
    use async_trait::async_trait;
    use ulid::Ulid;

    use super::*;
    use crate::asset::{AssetManager, TokenList};
    use crate::deps::ledger::{
        BudgetSource, BudgetSourceError, Clock, LedgerStore, LedgerStoreError,
    };
    use crate::deps::rebate::{RebateCallBuilderError, RebateMarketBookError, RebateStoreError};
    use crate::deps::routing::{GasPrice, GasPriceError};
    use crate::primitives::ledger::Reservation;
    use crate::primitives::rebate::{RebateExecution, RebatePlan};
    use crate::primitives::registry::MakerStrategy;
    use crate::primitives::trade::TradeId;
    use crate::primitives::{MakerId, StrategyHash};
    use crate::rebate::RebatePolicyConfig;
    use crate::registry::SharedSnapshot;

    struct EmptyBudget;

    #[async_trait]
    impl BudgetSource for EmptyBudget {
        async fn budget(
            &self,
            _account: &crate::primitives::ledger::AccountKey,
        ) -> Result<U256, BudgetSourceError> {
            Ok(U256::ZERO)
        }
    }

    struct FixedClock;

    impl Clock for FixedClock {
        fn now_unix(&self) -> u64 {
            1_000
        }
    }

    struct EmptyStore;

    struct OpenBatchStore(RebateBatch);

    struct FailingRebateStore;

    #[async_trait]
    impl LedgerStore for EmptyStore {
        async fn reserve(&self, _reservation: &Reservation) -> Result<(), LedgerStoreError> {
            Ok(())
        }

        async fn post(&self, _id: ReservationId, _filled: &[U256]) -> Result<(), LedgerStoreError> {
            Ok(())
        }

        async fn void(&self, _id: ReservationId) -> Result<(), LedgerStoreError> {
            Ok(())
        }

        async fn expire(&self, _id: ReservationId) -> Result<(), LedgerStoreError> {
            Ok(())
        }

        async fn void_reorg(&self, _id: ReservationId) -> Result<(), LedgerStoreError> {
            Ok(())
        }

        async fn open_reservations(&self) -> Result<Vec<Reservation>, LedgerStoreError> {
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl RebateStore for EmptyStore {
        async fn save(&self, _batch: &RebateBatch) -> Result<(), RebateStoreError> {
            Ok(())
        }

        async fn open_batches(&self) -> Result<Vec<RebateBatch>, RebateStoreError> {
            Ok(Vec::new())
        }

        async fn close(&self, _id: RebateBatchId) -> Result<(), RebateStoreError> {
            Ok(())
        }
    }

    #[async_trait]
    impl RebateStore for OpenBatchStore {
        async fn save(&self, _batch: &RebateBatch) -> Result<(), RebateStoreError> {
            Ok(())
        }

        async fn open_batches(&self) -> Result<Vec<RebateBatch>, RebateStoreError> {
            Ok(vec![self.0.clone()])
        }

        async fn close(&self, _id: RebateBatchId) -> Result<(), RebateStoreError> {
            Ok(())
        }
    }

    #[async_trait]
    impl RebateStore for FailingRebateStore {
        async fn save(&self, _batch: &RebateBatch) -> Result<(), RebateStoreError> {
            Err(RebateStoreError::Db("unavailable".to_string()))
        }

        async fn open_batches(&self) -> Result<Vec<RebateBatch>, RebateStoreError> {
            Err(RebateStoreError::Db("unavailable".to_string()))
        }

        async fn close(&self, _id: RebateBatchId) -> Result<(), RebateStoreError> {
            Err(RebateStoreError::Db("unavailable".to_string()))
        }
    }

    #[async_trait]
    impl RebateCallBuilder for EmptyStore {
        async fn build(
            &self,
            _strategy: &MakerStrategy,
            _plan: RebatePlan,
            _reservation: ReservationId,
            _deadline_block: u64,
            _published_at: u64,
        ) -> Result<RebateExecution, RebateCallBuilderError> {
            Err(RebateCallBuilderError::UndecodableProgram)
        }
    }

    #[async_trait]
    impl RebateMarketBook for EmptyStore {
        async fn market(
            &self,
            request: RebateMarketRequest,
        ) -> Result<crate::primitives::rebate::RebateMarket, RebateMarketBookError> {
            Ok(crate::primitives::rebate::RebateMarket::new(
                request.token_a,
                request.token_b,
                Ratio::new(U256::from(2_000u64), U256::from(1u64)).unwrap(),
                Ratio::new(U256::from(1u64), U256::from(2_100u64)).unwrap(),
            ))
        }
    }

    #[async_trait]
    impl GasPrice for EmptyStore {
        async fn gas_price_wei(&self) -> Result<u128, GasPriceError> {
            Ok(1)
        }
    }

    fn strategy_key() -> StrategyKey {
        StrategyKey {
            maker: MakerId(Address::from([1; 20])),
            app: Address::from([2; 20]),
            strategy_hash: StrategyHash(B256::from([3; 32])),
        }
    }

    fn accrual(amount_in: u64) -> RebateAccrual {
        RebateAccrual::new(
            TradeId(Ulid::from_parts(1, 1)),
            strategy_key(),
            Address::from([4; 20]),
            Address::from([5; 20]),
            U256::from(amount_in),
            U256::from(9u64),
            U256::from(10u64),
        )
    }

    fn service(guards: Arc<StrategyGuard>) -> RebateService {
        service_with_store(guards, Arc::new(EmptyStore))
    }

    fn service_with_store(
        guards: Arc<StrategyGuard>,
        store: Arc<dyn RebateStore>,
    ) -> RebateService {
        let ledger = Arc::new(LedgerService::new(
            Arc::new(EmptyStore),
            Arc::new(EmptyBudget),
            Arc::new(FixedClock),
        ));
        let assets = Arc::new(AssetManager::new(
            TokenList {
                name: "test".to_string(),
                tokens: Vec::new(),
            },
            Arc::new(SharedSnapshot::default()),
        ));
        RebateService::new(
            RebatePolicy::new(RebatePolicyConfig::new(100, 10_000)),
            ledger,
            guards,
            store,
            Arc::new(EmptyStore),
            RebateMarketData::new(Arc::new(EmptyStore), Arc::new(EmptyStore), assets),
            RebateServiceConfig::new(Address::ZERO, 1, 1),
        )
    }

    #[tokio::test]
    async fn duplicate_accrual_does_not_restore_a_released_guard() {
        let guards = Arc::new(StrategyGuard::default());
        let service = service(Arc::clone(&guards));
        let original = accrual(10);

        let batch = service.accrue(original.clone()).await.unwrap();
        assert!(guards.is_guarded(&strategy_key()));
        service
            .batches
            .lock()
            .await
            .get_mut(&strategy_key())
            .unwrap()
            .state = RebateBatchState::Accumulating;
        guards.unguard(&strategy_key());

        assert_eq!(service.accrue(original).await.unwrap(), batch);
        assert!(!guards.is_guarded(&strategy_key()));

        let error = service.accrue(accrual(11)).await.unwrap_err();
        assert!(matches!(
            error,
            SolventError::Rebate(RebateError::InvalidAccrual(_))
        ));
        assert!(!guards.is_guarded(&strategy_key()));
    }

    #[tokio::test]
    async fn new_accrual_stays_guarded_until_market_evaluation() {
        let guards = Arc::new(StrategyGuard::default());
        let service = service(Arc::clone(&guards));

        service.accrue(accrual(10)).await.unwrap();

        assert!(guards.is_guarded(&strategy_key()));
        assert!(matches!(
            service.batches.lock().await[&strategy_key()].state,
            RebateBatchState::Guarded
        ));
    }

    #[tokio::test]
    async fn gas_uses_the_adverse_executable_conversion_side() {
        let service = service(Arc::new(StrategyGuard::default()));

        assert_eq!(
            service
                .gas_cost_in(U256::from(1_000u64), Address::from([4; 20]))
                .await
                .unwrap(),
            U256::from(2_100_000u64)
        );
    }

    #[tokio::test]
    async fn recovery_reconstructs_a_persisted_strategy_guard() {
        let guards = Arc::new(StrategyGuard::default());
        let original = accrual(10);
        let mut batch = RebateBatch::new(
            batch_id(&original),
            TokenPair::new(original.token_in, original.token_out),
            original,
        );
        batch.state = RebateBatchState::Guarded;
        let service =
            service_with_store(Arc::clone(&guards), Arc::new(OpenBatchStore(batch.clone())));

        service.recover().await.unwrap();

        assert!(guards.is_guarded(&batch.strategy));
        assert_eq!(
            service.batches.lock().await.get(&batch.strategy),
            Some(&batch)
        );
    }

    #[tokio::test]
    async fn failed_first_persistence_does_not_strand_a_strategy_guard() {
        let guards = Arc::new(StrategyGuard::default());
        let service = service_with_store(Arc::clone(&guards), Arc::new(FailingRebateStore));

        let error = service.accrue(accrual(10)).await.unwrap_err();

        assert!(matches!(error, SolventError::RebateStore(_)));
        assert!(!guards.is_guarded(&strategy_key()));
    }
}
