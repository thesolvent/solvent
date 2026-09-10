//! SQLite persistence for the one-open-batch-per-strategy rebate state machine.

use std::collections::BTreeMap;
use std::str::FromStr;

use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::sol_types::{SolCall, SolValue};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use solvent_core::deps::rebate::{ExecutedRebateQuery, RebateStore, RebateStoreError};
use solvent_core::primitives::execution::{ExecutionAuthorization, ExecutionKind, PolicySignature};
use solvent_core::primitives::rebate::{
    RebateAccrual, RebateAllocation, RebateBatch, RebateBatchState, RebateExecutedEvent,
    RebateExecution, RebatePlan, RebateSettlement,
};
use solvent_core::primitives::registry::{StrategyKey, TokenPair};
use solvent_core::primitives::trade::TradeId;
use solvent_core::primitives::{ChainId, MakerId, RebateBatchId, ReservationId, StrategyHash};
use sqlx::{QueryBuilder, Sqlite, SqlitePool, Transaction};

use crate::execution::filler::{executeRebateCall, Authorization, Order};

pub struct SqliteRebateStore {
    pool: SqlitePool,
}

impl SqliteRebateStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn persist_accrual(
        transaction: &mut Transaction<'_, Sqlite>,
        batch: &RebateBatch,
        accrual: &RebateAccrual,
    ) -> Result<(), RebateStoreError> {
        let payload = serde_json::to_string(&StoredAccrual::from(accrual)).map_err(db)?;
        let inserted = sqlx::query(
            "INSERT INTO rebate_accrual
             (batch_id, maker, app, strategy_hash, trade_id, payload)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT DO NOTHING",
        )
        .bind(batch.id.0.to_vec())
        .bind(batch.strategy.maker.0.to_vec())
        .bind(batch.strategy.app.to_vec())
        .bind(batch.strategy.strategy_hash.0.to_vec())
        .bind(accrual.trade_id.to_string())
        .bind(&payload)
        .execute(&mut **transaction)
        .await
        .map_err(db)?
        .rows_affected();
        if inserted == 1 {
            return Ok(());
        }

        let stored: Option<(Vec<u8>, String)> = sqlx::query_as(
            "SELECT batch_id, payload FROM rebate_accrual
             WHERE maker = ? AND app = ? AND strategy_hash = ? AND trade_id = ?",
        )
        .bind(batch.strategy.maker.0.to_vec())
        .bind(batch.strategy.app.to_vec())
        .bind(batch.strategy.strategy_hash.0.to_vec())
        .bind(accrual.trade_id.to_string())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(db)?;
        let exact = stored.is_some_and(|(batch_id, stored)| {
            batch_id == batch.id.0.as_slice() && stored == payload
        });
        match exact {
            true => Ok(()),
            false => Err(RebateStoreError::Conflict(batch.strategy.strategy_hash)),
        }
    }
}

#[async_trait]
impl RebateStore for SqliteRebateStore {
    async fn save(&self, batch: &RebateBatch) -> Result<(), RebateStoreError> {
        let mut transaction = self.pool.begin().await.map_err(db)?;
        sqlx::query(
            "INSERT INTO rebate_batch
             (id, maker, app, strategy_hash, token_lo, token_hi, state, state_payload, is_open)
             VALUES (?, ?, ?, ?, ?, ?, 'accumulating', NULL, 1)
             ON CONFLICT DO NOTHING",
        )
        .bind(batch.id.0.to_vec())
        .bind(batch.strategy.maker.0.to_vec())
        .bind(batch.strategy.app.to_vec())
        .bind(batch.strategy.strategy_hash.0.to_vec())
        .bind(batch.pair.lo.to_vec())
        .bind(batch.pair.hi.to_vec())
        .execute(&mut *transaction)
        .await
        .map_err(db)?;

        let stored: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = sqlx::query_as(
            "SELECT id, token_lo, token_hi FROM rebate_batch
             WHERE maker = ? AND app = ? AND strategy_hash = ? AND is_open = 1",
        )
        .bind(batch.strategy.maker.0.to_vec())
        .bind(batch.strategy.app.to_vec())
        .bind(batch.strategy.strategy_hash.0.to_vec())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(db)?;
        let exact = stored.is_some_and(|(id, lo, hi)| {
            id == batch.id.0.as_slice()
                && lo == batch.pair.lo.as_slice()
                && hi == batch.pair.hi.as_slice()
        });
        if !exact {
            return Err(RebateStoreError::Conflict(batch.strategy.strategy_hash));
        }

        for accrual in batch.accruals.values() {
            Self::persist_accrual(&mut transaction, batch, accrual).await?;
        }
        let stored_state = StoredState::try_from(&batch.state)?;
        let state = stored_state.name();
        let state_payload = serde_json::to_string(&stored_state).map_err(db)?;
        let updated = sqlx::query(
            "UPDATE rebate_batch SET state = ?, state_payload = ?
             WHERE id = ? AND is_open = 1",
        )
        .bind(state)
        .bind(state_payload)
        .bind(batch.id.0.to_vec())
        .execute(&mut *transaction)
        .await
        .map_err(db)?
        .rows_affected();
        if updated != 1 {
            return Err(RebateStoreError::Conflict(batch.strategy.strategy_hash));
        }
        transaction.commit().await.map_err(db)
    }

    async fn open_batches(&self) -> Result<Vec<RebateBatch>, RebateStoreError> {
        let rows: Vec<BatchRow> = sqlx::query_as(
            "SELECT id, maker, app, strategy_hash, token_lo, token_hi, state, state_payload
             FROM rebate_batch WHERE is_open = 1 ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        let mut batches = Vec::with_capacity(rows.len());
        for row in rows {
            batches.push(row.into_batch(&self.pool).await?);
        }
        Ok(batches)
    }

    async fn close(&self, id: RebateBatchId) -> Result<(), RebateStoreError> {
        sqlx::query(
            "UPDATE rebate_batch
             SET state = 'closed', state_payload = NULL, is_open = 0
             WHERE id = ? AND is_open = 1",
        )
        .bind(id.0.to_vec())
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn scan_cursor(&self, chain: ChainId) -> Result<Option<u64>, RebateStoreError> {
        let value: Option<(i64,)> =
            sqlx::query_as("SELECT block_number FROM rebate_scan_cursor WHERE chain = ?")
                .bind(i64_of(chain.0)?)
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?;
        value
            .map(|(block,)| u64_of(block, "rebate scan block"))
            .transpose()
    }

    async fn save_scan_cursor(
        &self,
        chain: ChainId,
        block_number: u64,
    ) -> Result<(), RebateStoreError> {
        sqlx::query(
            "INSERT INTO rebate_scan_cursor (chain, block_number) VALUES (?, ?)
             ON CONFLICT (chain) DO UPDATE SET block_number = excluded.block_number
             WHERE rebate_scan_cursor.block_number <= excluded.block_number",
        )
        .bind(i64_of(chain.0)?)
        .bind(i64_of(block_number)?)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn begin_settlement(
        &self,
        settlement: &RebateSettlement,
    ) -> Result<(), RebateStoreError> {
        let payload = serde_json::to_string(&StoredSettlement::from(settlement)).map_err(db)?;
        sqlx::query(
            "INSERT INTO rebate_settlement (batch_id, status, payload, maker, executed_at)
             VALUES (?, 'settling', ?, ?, ?) ON CONFLICT DO NOTHING",
        )
        .bind(settlement.plan.batch_id.0.to_vec())
        .bind(&payload)
        .bind(settlement.plan.strategy.maker.0.to_vec())
        .bind(i64_of(settlement.executed_at)?)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        let stored: Option<(String,)> =
            sqlx::query_as("SELECT payload FROM rebate_settlement WHERE batch_id = ?")
                .bind(settlement.plan.batch_id.0.to_vec())
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?;
        let exact = stored
            .map(|(stored,)| serde_json::from_str::<StoredSettlement>(&stored).map_err(db))
            .transpose()?
            .map(StoredSettlement::into_domain)
            .transpose()?
            .is_some_and(|stored| {
                stored.plan == settlement.plan
                    && stored.reservation == settlement.reservation
                    && stored.event == settlement.event
            });
        match exact {
            true => Ok(()),
            false => Err(RebateStoreError::Conflict(
                settlement.plan.strategy.strategy_hash,
            )),
        }
    }

    async fn pending_settlements(&self) -> Result<Vec<RebateSettlement>, RebateStoreError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT payload FROM rebate_settlement WHERE status = 'settling' ORDER BY batch_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|(payload,)| {
                serde_json::from_str::<StoredSettlement>(&payload)
                    .map_err(db)?
                    .into_domain()
            })
            .collect()
    }

    async fn finish_settlement(&self, id: RebateBatchId) -> Result<(), RebateStoreError> {
        let mut transaction = self.pool.begin().await.map_err(db)?;
        let updated = sqlx::query(
            "UPDATE rebate_settlement SET status = 'executed'
             WHERE batch_id = ? AND status = 'settling'",
        )
        .bind(id.0.to_vec())
        .execute(&mut *transaction)
        .await
        .map_err(db)?
        .rows_affected();
        if updated == 0 {
            let status: Option<(String,)> =
                sqlx::query_as("SELECT status FROM rebate_settlement WHERE batch_id = ?")
                    .bind(id.0.to_vec())
                    .fetch_optional(&mut *transaction)
                    .await
                    .map_err(db)?;
            if !status.is_some_and(|(status,)| status == "executed") {
                return Err(RebateStoreError::Db(
                    "rebate settlement does not exist".to_string(),
                ));
            }
        }
        sqlx::query(
            "UPDATE rebate_batch
             SET state = 'closed', state_payload = NULL, is_open = 0 WHERE id = ?",
        )
        .bind(id.0.to_vec())
        .execute(&mut *transaction)
        .await
        .map_err(db)?;
        transaction.commit().await.map_err(db)
    }

    async fn executed_rebates(
        &self,
        query: &ExecutedRebateQuery,
    ) -> Result<Vec<RebateSettlement>, RebateStoreError> {
        let mut statement = QueryBuilder::<Sqlite>::new(
            "SELECT payload FROM rebate_settlement WHERE status = 'executed'",
        );
        if let Some(maker) = query.maker {
            statement.push(" AND maker = ").push_bind(maker.0.to_vec());
        }
        if let Some(cursor) = query.before {
            let executed_at = i64_of(cursor.executed_at)?;
            statement
                .push(" AND (executed_at < ")
                .push_bind(executed_at)
                .push(" OR (executed_at = ")
                .push_bind(executed_at)
                .push(" AND batch_id < ")
                .push_bind(cursor.batch_id.0.to_vec())
                .push("))");
        }
        statement
            .push(" ORDER BY executed_at DESC, batch_id DESC LIMIT ")
            .push_bind(i64::from(query.limit));
        let rows = statement
            .build_query_as::<(String,)>()
            .fetch_all(&self.pool)
            .await
            .map_err(db)?;
        rows.into_iter()
            .map(|(payload,)| decode_settlement(&payload))
            .collect()
    }

    async fn executed_rebate(
        &self,
        id: RebateBatchId,
    ) -> Result<Option<RebateSettlement>, RebateStoreError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT payload FROM rebate_settlement
             WHERE batch_id = ? AND status = 'executed'",
        )
        .bind(id.0.to_vec())
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        row.map(|(payload,)| decode_settlement(&payload))
            .transpose()
    }
}

#[derive(sqlx::FromRow)]
struct BatchRow {
    id: Vec<u8>,
    maker: Vec<u8>,
    app: Vec<u8>,
    strategy_hash: Vec<u8>,
    token_lo: Vec<u8>,
    token_hi: Vec<u8>,
    state: String,
    state_payload: Option<String>,
}

impl BatchRow {
    async fn into_batch(self, pool: &SqlitePool) -> Result<RebateBatch, RebateStoreError> {
        let id = RebateBatchId(b256(&self.id, "batch id")?);
        let strategy = StrategyKey {
            maker: MakerId(address(&self.maker, "maker")?),
            app: address(&self.app, "app")?,
            strategy_hash: StrategyHash(b256(&self.strategy_hash, "strategy hash")?),
        };
        let pair = TokenPair::new(
            address(&self.token_lo, "token lo")?,
            address(&self.token_hi, "token hi")?,
        );
        let payload = self
            .state_payload
            .ok_or_else(|| RebateStoreError::Db("open batch has no state payload".to_string()))?;
        let stored: StoredState = serde_json::from_str(&payload).map_err(db)?;
        if stored.name() != self.state {
            return Err(RebateStoreError::Db(
                "rebate state label does not match its payload".to_string(),
            ));
        }
        let state = stored.into_domain()?;
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT payload FROM rebate_accrual WHERE batch_id = ? ORDER BY trade_id",
        )
        .bind(id.0.to_vec())
        .fetch_all(pool)
        .await
        .map_err(db)?;
        let mut accruals = BTreeMap::new();
        for (payload,) in rows {
            let accrual = serde_json::from_str::<StoredAccrual>(&payload)
                .map_err(db)?
                .into_domain()?;
            if accrual.strategy != strategy {
                return Err(RebateStoreError::Db(
                    "accrual strategy does not match its batch".to_string(),
                ));
            }
            accruals.insert(accrual.trade_id, accrual);
        }
        Ok(RebateBatch::from_parts(id, strategy, pair, accruals, state))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum StoredState {
    Accumulating,
    Guarded,
    Preparing {
        plan: Box<StoredPlan>,
        reservation: B256,
    },
    Ready {
        execution: Box<StoredExecution>,
    },
    Invalidating {
        reservation: B256,
    },
}

impl StoredState {
    fn name(&self) -> &'static str {
        match self {
            Self::Accumulating => "accumulating",
            Self::Guarded => "guarded",
            Self::Preparing { .. } => "preparing",
            Self::Ready { .. } => "ready",
            Self::Invalidating { .. } => "invalidating",
        }
    }

    fn into_domain(self) -> Result<RebateBatchState, RebateStoreError> {
        Ok(match self {
            Self::Accumulating => RebateBatchState::Accumulating,
            Self::Guarded => RebateBatchState::Guarded,
            Self::Preparing { plan, reservation } => RebateBatchState::Preparing {
                plan: Box::new(plan.into_domain()?),
                reservation: ReservationId(reservation),
            },
            Self::Ready { execution } => {
                RebateBatchState::Ready(Box::new(execution.into_domain()?))
            }
            Self::Invalidating { reservation } => RebateBatchState::Invalidating {
                reservation: ReservationId(reservation),
            },
        })
    }
}

impl TryFrom<&RebateBatchState> for StoredState {
    type Error = RebateStoreError;

    fn try_from(state: &RebateBatchState) -> Result<Self, Self::Error> {
        Ok(match state {
            RebateBatchState::Accumulating => Self::Accumulating,
            RebateBatchState::Guarded => Self::Guarded,
            RebateBatchState::Preparing { plan, reservation } => Self::Preparing {
                plan: Box::new(StoredPlan::from(plan.as_ref())),
                reservation: reservation.0,
            },
            RebateBatchState::Ready(execution) => Self::Ready {
                execution: Box::new(StoredExecution::from(execution.as_ref())),
            },
            RebateBatchState::Invalidating { reservation } => Self::Invalidating {
                reservation: reservation.0,
            },
            _ => {
                return Err(RebateStoreError::Db(
                    "unsupported rebate batch state".to_string(),
                ))
            }
        })
    }
}

#[derive(Serialize, Deserialize)]
struct StoredExecution {
    plan: StoredPlan,
    reservation: B256,
    authorization: StoredAuthorization,
    order: Bytes,
    policy_signature: Bytes,
    calldata: Bytes,
    published_at: u64,
}

impl StoredExecution {
    fn into_domain(self) -> Result<RebateExecution, RebateStoreError> {
        let authorization = self.authorization.into_domain()?;
        if self.policy_signature.len() != 65 {
            return Err(RebateStoreError::Db(
                "stored rebate signature must be 65 bytes".to_string(),
            ));
        }
        let order = Order::abi_decode(&self.order)
            .map_err(|error| RebateStoreError::Db(format!("stored rebate order: {error}")))?;
        if order.maker != authorization.maker.0
            || keccak256(order.abi_encode()) != authorization.strategy_hash.0
        {
            return Err(RebateStoreError::Db(
                "stored rebate order does not match its authorization".to_string(),
            ));
        }
        let expected_calldata = Bytes::from(
            executeRebateCall {
                order,
                authorization: Authorization::from(&authorization),
                signature: self.policy_signature.clone(),
            }
            .abi_encode(),
        );
        if expected_calldata != self.calldata {
            return Err(RebateStoreError::Db(
                "stored rebate calldata does not match its signed payload".to_string(),
            ));
        }
        Ok(RebateExecution::new(
            self.plan.into_domain()?,
            ReservationId(self.reservation),
            authorization,
            self.order,
            PolicySignature::new(self.policy_signature),
            self.calldata,
            self.published_at,
        ))
    }
}

impl From<&RebateExecution> for StoredExecution {
    fn from(execution: &RebateExecution) -> Self {
        Self {
            plan: StoredPlan::from(execution.plan.as_ref()),
            reservation: execution.reservation.0,
            authorization: StoredAuthorization::from(&execution.authorization),
            order: execution.order.clone(),
            policy_signature: execution.policy_signature.as_bytes().clone(),
            calldata: execution.calldata.clone(),
            published_at: execution.published_at,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct StoredAuthorization {
    kind: u8,
    nonce: U256,
    context_hash: B256,
    strategy_hash: B256,
    maker: Address,
    token_in: Address,
    token_out: Address,
    amount_out: U256,
    amount_in_limit: U256,
    rebate_amount: U256,
    deadline_block: u64,
}

impl StoredAuthorization {
    fn into_domain(self) -> Result<ExecutionAuthorization, RebateStoreError> {
        if self.kind != ExecutionKind::Rebate.as_u8() {
            return Err(RebateStoreError::Db(
                "stored rebate authorization has the wrong execution kind".to_string(),
            ));
        }
        Ok(ExecutionAuthorization::rebate(
            solvent_core::primitives::execution::RebateAuthorization {
                nonce: self.nonce,
                context_hash: self.context_hash,
                strategy_hash: StrategyHash(self.strategy_hash),
                maker: MakerId(self.maker),
                token_in: self.token_in,
                token_out: self.token_out,
                amount_out: self.amount_out,
                amount_in_limit: self.amount_in_limit,
                rebate_amount: self.rebate_amount,
                deadline_block: self.deadline_block,
            },
        ))
    }
}

impl From<&ExecutionAuthorization> for StoredAuthorization {
    fn from(value: &ExecutionAuthorization) -> Self {
        Self {
            kind: value.kind.as_u8(),
            nonce: value.nonce,
            context_hash: value.context_hash,
            strategy_hash: value.strategy_hash.0,
            maker: value.maker.0,
            token_in: value.token_in,
            token_out: value.token_out,
            amount_out: value.amount_out,
            amount_in_limit: value.amount_in_limit,
            rebate_amount: value.rebate_amount,
            deadline_block: value.deadline_block,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct StoredPlan {
    batch_id: B256,
    maker: Address,
    app: Address,
    strategy_hash: B256,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    amount_out: U256,
    gross_surplus: U256,
    safe_gas_cost: U256,
    maker_rebate: U256,
    executor_profit: U256,
    deviation_bps: u64,
    allocations: Vec<StoredAllocation>,
}

impl StoredPlan {
    fn into_domain(self) -> Result<RebatePlan, RebateStoreError> {
        Ok(RebatePlan::new(
            RebateBatchId(self.batch_id),
            StrategyKey {
                maker: MakerId(self.maker),
                app: self.app,
                strategy_hash: StrategyHash(self.strategy_hash),
            },
            self.token_in,
            self.token_out,
            self.amount_in,
            self.amount_out,
            self.gross_surplus,
            self.safe_gas_cost,
            self.maker_rebate,
            self.executor_profit,
            self.deviation_bps,
            self.allocations
                .into_iter()
                .map(StoredAllocation::into_domain)
                .collect::<Result<_, _>>()?,
        ))
    }
}

impl From<&RebatePlan> for StoredPlan {
    fn from(plan: &RebatePlan) -> Self {
        Self {
            batch_id: plan.batch_id.0,
            maker: plan.strategy.maker.0,
            app: plan.strategy.app,
            strategy_hash: plan.strategy.strategy_hash.0,
            token_in: plan.token_in,
            token_out: plan.token_out,
            amount_in: plan.amount_in,
            amount_out: plan.amount_out,
            gross_surplus: plan.gross_surplus,
            safe_gas_cost: plan.safe_gas_cost,
            maker_rebate: plan.maker_rebate,
            executor_profit: plan.executor_profit,
            deviation_bps: plan.deviation_bps,
            allocations: plan
                .allocations
                .iter()
                .map(StoredAllocation::from)
                .collect(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct StoredAllocation {
    trade_id: String,
    amount: U256,
}

impl StoredAllocation {
    fn into_domain(self) -> Result<RebateAllocation, RebateStoreError> {
        Ok(RebateAllocation::new(
            TradeId::from_str(&self.trade_id).map_err(db)?,
            self.amount,
        ))
    }
}

impl From<&RebateAllocation> for StoredAllocation {
    fn from(value: &RebateAllocation) -> Self {
        Self {
            trade_id: value.trade_id.to_string(),
            amount: value.amount,
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct StoredAccrual {
    trade_id: String,
    maker: Address,
    app: Address,
    strategy_hash: B256,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    amount_out: U256,
    allocation_weight: U256,
    confirmed_block: u64,
}

impl StoredAccrual {
    fn into_domain(self) -> Result<RebateAccrual, RebateStoreError> {
        Ok(RebateAccrual::new(
            TradeId::from_str(&self.trade_id).map_err(db)?,
            StrategyKey {
                maker: MakerId(self.maker),
                app: self.app,
                strategy_hash: StrategyHash(self.strategy_hash),
            },
            self.token_in,
            self.token_out,
            self.amount_in,
            self.amount_out,
            self.allocation_weight,
            self.confirmed_block,
        ))
    }
}

impl From<&RebateAccrual> for StoredAccrual {
    fn from(value: &RebateAccrual) -> Self {
        Self {
            trade_id: value.trade_id.to_string(),
            maker: value.strategy.maker.0,
            app: value.strategy.app,
            strategy_hash: value.strategy.strategy_hash.0,
            token_in: value.token_in,
            token_out: value.token_out,
            amount_in: value.amount_in,
            amount_out: value.amount_out,
            allocation_weight: value.allocation_weight,
            confirmed_block: value.confirmed_block,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct StoredSettlement {
    plan: StoredPlan,
    reservation: B256,
    event: StoredExecutedEvent,
    executed_at: u64,
}

impl StoredSettlement {
    fn into_domain(self) -> Result<RebateSettlement, RebateStoreError> {
        Ok(RebateSettlement::new(
            self.plan.into_domain()?,
            ReservationId(self.reservation),
            self.event.into_domain(),
            self.executed_at,
        ))
    }
}

impl From<&RebateSettlement> for StoredSettlement {
    fn from(value: &RebateSettlement) -> Self {
        Self {
            plan: StoredPlan::from(value.plan.as_ref()),
            reservation: value.reservation.0,
            event: StoredExecutedEvent::from(&value.event),
            executed_at: value.executed_at,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct StoredExecutedEvent {
    batch_id: B256,
    strategy_hash: B256,
    executor: Address,
    maker: Address,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    amount_out: U256,
    maker_rebate: U256,
    tx_hash: B256,
    block_number: u64,
    log_index: u64,
}

impl StoredExecutedEvent {
    fn into_domain(self) -> RebateExecutedEvent {
        RebateExecutedEvent::new(
            RebateBatchId(self.batch_id),
            StrategyHash(self.strategy_hash),
            self.executor,
            MakerId(self.maker),
            self.token_in,
            self.token_out,
            self.amount_in,
            self.amount_out,
            self.maker_rebate,
            self.tx_hash,
            self.block_number,
            self.log_index,
        )
    }
}

impl From<&RebateExecutedEvent> for StoredExecutedEvent {
    fn from(value: &RebateExecutedEvent) -> Self {
        Self {
            batch_id: value.batch_id.0,
            strategy_hash: value.strategy_hash.0,
            executor: value.executor,
            maker: value.maker.0,
            token_in: value.token_in,
            token_out: value.token_out,
            amount_in: value.amount_in,
            amount_out: value.amount_out,
            maker_rebate: value.maker_rebate,
            tx_hash: value.tx_hash,
            block_number: value.block_number,
            log_index: value.log_index,
        }
    }
}

fn b256(bytes: &[u8], field: &str) -> Result<B256, RebateStoreError> {
    B256::try_from(bytes)
        .map_err(|_| RebateStoreError::Db(format!("{field} must contain 32 bytes")))
}

fn address(bytes: &[u8], field: &str) -> Result<Address, RebateStoreError> {
    Address::try_from(bytes)
        .map_err(|_| RebateStoreError::Db(format!("{field} must contain 20 bytes")))
}

fn i64_of(value: u64) -> Result<i64, RebateStoreError> {
    i64::try_from(value).map_err(|_| db(format!("value {value} exceeds i64")))
}

fn u64_of(value: i64, field: &str) -> Result<u64, RebateStoreError> {
    u64::try_from(value).map_err(|_| db(format!("{field} cannot be negative")))
}

fn decode_settlement(payload: &str) -> Result<RebateSettlement, RebateStoreError> {
    serde_json::from_str::<StoredSettlement>(payload)
        .map_err(db)?
        .into_domain()
}

fn db(error: impl std::fmt::Display) -> RebateStoreError {
    RebateStoreError::Db(error.to_string())
}
