use alloy::primitives::{Address, B256};
use async_trait::async_trait;
use solvent_core::deps::crosschain::{
    LegQuoteStore, LegQuoteStoreError, PreparationStore, PreparationStoreError, SagaStore,
    SagaStoreError, StepStore, StepStoreError,
};
use solvent_core::primitives::crosschain::{
    ChainExecutionPlan, CrossChainSaga, LegRole, Preparation, PreparationState, PreparedStep,
    RemoteCommand, SagaState,
};
use solvent_core::primitives::{AggregateQuoteId, CrossChainOrderId, PrepareToken};
use sqlx::SqlitePool;

pub struct SqlitePreparationStore {
    pool: SqlitePool,
}

pub struct SqliteLegQuoteStore {
    pool: SqlitePool,
}

impl SqliteLegQuoteStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl SqlitePreparationStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn migrate(&self) -> Result<(), PreparationStoreError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(preparation_db)
    }
}

pub struct SqliteSagaStore {
    pool: SqlitePool,
}

pub struct SqliteStepStore {
    pool: SqlitePool,
}

impl SqliteStepStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl SqliteSagaStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn migrate(&self) -> Result<(), SagaStoreError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(saga_db)
    }
}

#[async_trait]
impl PreparationStore for SqlitePreparationStore {
    async fn insert(
        &self,
        preparation: &Preparation,
    ) -> Result<Preparation, PreparationStoreError> {
        let body = serde_json::to_string(preparation).map_err(preparation_db)?;
        sqlx::query(
            "INSERT INTO crosschain_preparation
             (token, quote_id, role, chain_id, state, expires_at, body)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(token) DO NOTHING",
        )
        .bind(preparation.token.0.to_vec())
        .bind(preparation.quote_id.0.to_vec())
        .bind(role_label(preparation.role))
        .bind(i64_of(preparation.chain_id.0).map_err(preparation_db)?)
        .bind(preparation_state_label(preparation.state))
        .bind(i64_of(preparation.expires_at_unix).map_err(preparation_db)?)
        .bind(body)
        .execute(&self.pool)
        .await
        .map_err(preparation_db)?;

        let stored = self.load(preparation.token).await?;
        match stored {
            Some(stored) if stored == *preparation => Ok(stored),
            Some(_) => Err(PreparationStoreError::Conflict),
            None => Err(PreparationStoreError::Backend(
                "inserted preparation could not be read".to_string(),
            )),
        }
    }

    async fn load(
        &self,
        token: PrepareToken,
    ) -> Result<Option<Preparation>, PreparationStoreError> {
        let body: Option<String> =
            sqlx::query_scalar("SELECT body FROM crosschain_preparation WHERE token = ?")
                .bind(token.0.to_vec())
                .fetch_optional(&self.pool)
                .await
                .map_err(preparation_db)?;
        body.map(|body| serde_json::from_str(&body).map_err(preparation_db))
            .transpose()
    }

    async fn load_for_quote(
        &self,
        quote_id: AggregateQuoteId,
        role: LegRole,
    ) -> Result<Option<Preparation>, PreparationStoreError> {
        let body: Option<String> = sqlx::query_scalar(
            "SELECT body FROM crosschain_preparation WHERE quote_id = ? AND role = ?",
        )
        .bind(quote_id.0.to_vec())
        .bind(role_label(role))
        .fetch_optional(&self.pool)
        .await
        .map_err(preparation_db)?;
        body.map(|body| serde_json::from_str(&body).map_err(preparation_db))
            .transpose()
    }

    async fn update(&self, preparation: &Preparation) -> Result<(), PreparationStoreError> {
        let body = serde_json::to_string(preparation).map_err(preparation_db)?;
        let result =
            sqlx::query("UPDATE crosschain_preparation SET state = ?, body = ? WHERE token = ?")
                .bind(preparation_state_label(preparation.state))
                .bind(body)
                .bind(preparation.token.0.to_vec())
                .execute(&self.pool)
                .await
                .map_err(preparation_db)?;
        if result.rows_affected() != 1 {
            return Err(PreparationStoreError::Backend(
                "preparation disappeared during update".to_string(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl LegQuoteStore for SqliteLegQuoteStore {
    async fn insert(
        &self,
        quote: &solvent_core::primitives::crosschain::LegQuote,
    ) -> Result<solvent_core::primitives::crosschain::LegQuote, LegQuoteStoreError> {
        let body = serde_json::to_string(quote).map_err(quote_db)?;
        sqlx::query(
            "INSERT INTO crosschain_leg_quote (quote_id, expires_at, body) VALUES (?, ?, ?)
             ON CONFLICT(quote_id) DO NOTHING",
        )
        .bind(quote.quote_id.to_vec())
        .bind(i64_of(quote.expires_at_unix).map_err(quote_db)?)
        .bind(body)
        .execute(&self.pool)
        .await
        .map_err(quote_db)?;
        match self.load(quote.quote_id).await? {
            Some(stored) if stored == *quote => Ok(stored),
            Some(_) => Err(LegQuoteStoreError::Conflict),
            None => Err(LegQuoteStoreError::Backend(
                "inserted quote could not be read".to_string(),
            )),
        }
    }

    async fn load(
        &self,
        quote_id: B256,
    ) -> Result<Option<solvent_core::primitives::crosschain::LegQuote>, LegQuoteStoreError> {
        let body: Option<String> =
            sqlx::query_scalar("SELECT body FROM crosschain_leg_quote WHERE quote_id = ?")
                .bind(quote_id.to_vec())
                .fetch_optional(&self.pool)
                .await
                .map_err(quote_db)?;
        body.map(|body| serde_json::from_str(&body).map_err(quote_db))
            .transpose()
    }
}

/// How a taker is written to the indexed column: lower-case hex, matching the JSON body the
/// migration backfilled from, so a row written before and after the column agree.
fn taker_key(taker: Option<Address>) -> Option<String> {
    taker.map(|taker| taker.to_string().to_lowercase())
}

#[async_trait]
impl SagaStore for SqliteSagaStore {
    async fn insert(&self, saga: &CrossChainSaga) -> Result<CrossChainSaga, SagaStoreError> {
        let body = serde_json::to_string(saga).map_err(saga_db)?;
        sqlx::query(
            "INSERT INTO crosschain_saga (order_id, state, taker, body) VALUES (?, ?, ?, ?)
             ON CONFLICT(order_id) DO NOTHING",
        )
        .bind(saga.order_id.0.to_vec())
        .bind(saga_state_label(saga.state))
        .bind(taker_key(saga.taker))
        .bind(body)
        .execute(&self.pool)
        .await
        .map_err(saga_db)?;
        let stored = self.load(saga.order_id).await?;
        match stored {
            Some(stored) if stored == *saga => Ok(stored),
            Some(_) => Err(SagaStoreError::Conflict),
            None => Err(SagaStoreError::Backend(
                "inserted saga could not be read".to_string(),
            )),
        }
    }

    async fn load(
        &self,
        order_id: CrossChainOrderId,
    ) -> Result<Option<CrossChainSaga>, SagaStoreError> {
        let body: Option<String> =
            sqlx::query_scalar("SELECT body FROM crosschain_saga WHERE order_id = ?")
                .bind(order_id.0.to_vec())
                .fetch_optional(&self.pool)
                .await
                .map_err(saga_db)?;
        body.map(|body| serde_json::from_str(&body).map_err(saga_db))
            .transpose()
    }

    async fn update(&self, saga: &CrossChainSaga) -> Result<(), SagaStoreError> {
        let body = serde_json::to_string(saga).map_err(saga_db)?;
        let result = sqlx::query(
            "UPDATE crosschain_saga
             SET state = ?, body = ?, updated_at = unixepoch() WHERE order_id = ?",
        )
        .bind(saga_state_label(saga.state))
        .bind(body)
        .bind(saga.order_id.0.to_vec())
        .execute(&self.pool)
        .await
        .map_err(saga_db)?;
        if result.rows_affected() != 1 {
            return Err(SagaStoreError::Backend(
                "saga disappeared during update".to_string(),
            ));
        }
        Ok(())
    }

    async fn by_taker(
        &self,
        taker: Address,
        limit: u32,
    ) -> Result<Vec<CrossChainSaga>, SagaStoreError> {
        let bodies: Vec<String> = sqlx::query_scalar(
            "SELECT body FROM crosschain_saga
             WHERE taker = ?
             ORDER BY updated_at DESC, order_id
             LIMIT ?",
        )
        .bind(taker_key(Some(taker)))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(saga_db)?;
        bodies
            .into_iter()
            .map(|body| serde_json::from_str(&body).map_err(saga_db))
            .collect()
    }

    async fn recoverable(&self) -> Result<Vec<CrossChainSaga>, SagaStoreError> {
        let bodies: Vec<String> = sqlx::query_scalar(
            "SELECT body FROM crosschain_saga
             WHERE state NOT IN ('complete', 'failed_before_delivery')
             ORDER BY updated_at, order_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(saga_db)?;
        bodies
            .into_iter()
            .map(|body| serde_json::from_str(&body).map_err(saga_db))
            .collect()
    }
}

#[async_trait]
impl StepStore for SqliteStepStore {
    async fn stage(
        &self,
        order_id: CrossChainOrderId,
        plan: &ChainExecutionPlan,
    ) -> Result<(), StepStoreError> {
        let mut transaction = self.pool.begin().await.map_err(step_db)?;
        sqlx::query(
            "INSERT OR IGNORE INTO crosschain_order_binding (order_id, aggregate_id) VALUES (?, ?)",
        )
        .bind(order_id.0.to_vec())
        .bind(plan.aggregate_id.0.to_vec())
        .execute(&mut *transaction)
        .await
        .map_err(step_db)?;
        let bound: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT aggregate_id FROM crosschain_order_binding WHERE order_id = ?",
        )
        .bind(order_id.0.to_vec())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(step_db)?;
        let bound = bound
            .as_deref()
            .map(aggregate_quote_id)
            .transpose()?
            .ok_or(StepStoreError::Conflict)?;
        if bound != plan.aggregate_id {
            return Err(StepStoreError::Conflict);
        }
        for step in &plan.steps {
            let body = serde_json::to_string(step).map_err(step_db)?;
            sqlx::query(
                "INSERT INTO crosschain_step (order_id, command, aggregate_id, body) VALUES (?, ?, ?, ?)
                 ON CONFLICT(order_id, command) DO NOTHING",
            )
            .bind(order_id.0.to_vec())
            .bind(command_label(step.command))
            .bind(plan.aggregate_id.0.to_vec())
            .bind(body)
            .execute(&mut *transaction)
            .await
            .map_err(step_db)?;

            let (aggregate_id, stored): (Option<Vec<u8>>, String) = sqlx::query_as(
                "SELECT aggregate_id, body FROM crosschain_step WHERE order_id = ? AND command = ?",
            )
            .bind(order_id.0.to_vec())
            .bind(command_label(step.command))
            .fetch_one(&mut *transaction)
            .await
            .map_err(step_db)?;
            let aggregate_id = aggregate_id
                .as_deref()
                .map(aggregate_quote_id)
                .transpose()?
                .ok_or(StepStoreError::Conflict)?;
            let stored: PreparedStep = serde_json::from_str(&stored).map_err(step_db)?;
            if aggregate_id != plan.aggregate_id || stored != *step {
                return Err(StepStoreError::Conflict);
            }
        }
        transaction.commit().await.map_err(step_db)
    }

    async fn load(
        &self,
        order_id: CrossChainOrderId,
        command: RemoteCommand,
    ) -> Result<Option<PreparedStep>, StepStoreError> {
        let body: Option<String> = sqlx::query_scalar(
            "SELECT body FROM crosschain_step WHERE order_id = ? AND command = ?",
        )
        .bind(order_id.0.to_vec())
        .bind(command_label(command))
        .fetch_optional(&self.pool)
        .await
        .map_err(step_db)?;
        body.map(|body| serde_json::from_str(&body).map_err(step_db))
            .transpose()
    }

    async fn aggregate_id(
        &self,
        order_id: CrossChainOrderId,
        command: RemoteCommand,
    ) -> Result<Option<AggregateQuoteId>, StepStoreError> {
        let aggregate_id: Option<Option<Vec<u8>>> = sqlx::query_scalar(
            "SELECT aggregate_id FROM crosschain_step WHERE order_id = ? AND command = ?",
        )
        .bind(order_id.0.to_vec())
        .bind(command_label(command))
        .fetch_optional(&self.pool)
        .await
        .map_err(step_db)?;
        aggregate_id
            .flatten()
            .as_deref()
            .map(aggregate_quote_id)
            .transpose()
    }
}

fn aggregate_quote_id(bytes: &[u8]) -> Result<AggregateQuoteId, StepStoreError> {
    B256::try_from(bytes).map(AggregateQuoteId).map_err(|_| {
        StepStoreError::Backend(format!(
            "expected a 32-byte aggregate id, got {}",
            bytes.len()
        ))
    })
}

fn i64_of(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| format!("value {value} exceeds i64"))
}

fn preparation_db(error: impl std::fmt::Display) -> PreparationStoreError {
    PreparationStoreError::Backend(error.to_string())
}

fn quote_db(error: impl std::fmt::Display) -> LegQuoteStoreError {
    LegQuoteStoreError::Backend(error.to_string())
}

fn saga_db(error: impl std::fmt::Display) -> SagaStoreError {
    SagaStoreError::Backend(error.to_string())
}

fn step_db(error: impl std::fmt::Display) -> StepStoreError {
    StepStoreError::Backend(error.to_string())
}

fn role_label(role: LegRole) -> &'static str {
    match role {
        LegRole::Origin => "origin",
        LegRole::Destination => "destination",
    }
}

fn preparation_state_label(state: PreparationState) -> &'static str {
    match state {
        PreparationState::Prepared => "prepared",
        PreparationState::Committed => "committed",
        PreparationState::Executed => "executed",
        PreparationState::Released => "released",
    }
}

fn saga_state_label(state: SagaState) -> &'static str {
    match state {
        SagaState::Quoted => "quoted",
        SagaState::Preparing => "preparing",
        SagaState::Prepared => "prepared",
        SagaState::DestinationPending => "destination_pending",
        SagaState::DestinationFinalized => "destination_finalized",
        SagaState::FillProofPending => "fill_proof_pending",
        SagaState::OriginPending => "origin_pending",
        SagaState::OriginFinalized => "origin_finalized",
        SagaState::RepaymentPending => "repayment_pending",
        SagaState::Complete => "complete",
        SagaState::FailedBeforeDelivery => "failed_before_delivery",
        SagaState::NeedsReconcile => "needs_reconcile",
    }
}

fn command_label(command: RemoteCommand) -> &'static str {
    match command {
        RemoteCommand::Deliver => "deliver",
        RemoteCommand::DispatchFillProof => "dispatch_fill_proof",
        RemoteCommand::ClaimOrigin => "claim_origin",
        RemoteCommand::DispatchRepayment => "dispatch_repayment",
        RemoteCommand::CloseDestination => "close_destination",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, B256, U256};
    use solvent_core::primitives::crosschain::{CrossChainRoute, LegQuote, LegRole};
    use solvent_core::primitives::{ChainId, PrepareToken};

    #[tokio::test]
    async fn preparation_insert_and_transition_survive_reopen() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let store = SqlitePreparationStore::new(pool);
        store.migrate().await.unwrap();
        let mut preparation = Preparation {
            token: PrepareToken(B256::from([1; 32])),
            quote_id: AggregateQuoteId(B256::from([2; 32])),
            role: LegRole::Origin,
            chain_id: ChainId(1),
            state: PreparationState::Prepared,
            expires_at_unix: 100,
        };
        store.insert(&preparation).await.unwrap();
        preparation.commit().unwrap();
        store.update(&preparation).await.unwrap();
        assert_eq!(
            store.load(preparation.token).await.unwrap(),
            Some(preparation)
        );
    }

    #[tokio::test]
    async fn issued_leg_quote_round_trips_exact_terms() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let store = SqliteLegQuoteStore::new(pool.clone());
        SqlitePreparationStore::new(pool).migrate().await.unwrap();
        let mut quote = LegQuote {
            quote_id: B256::ZERO,
            request_id: B256::from([1; 32]),
            role: LegRole::Origin,
            local_chain: ChainId(1),
            remote_chain: ChainId(10),
            input_token: Address::from([2; 20]),
            output_token: Address::from([3; 20]),
            amount_in: U256::from(100),
            amount_out: U256::from(99),
            route: CrossChainRoute::Cctp,
            price_impact_bps: None,
            block_number: 7,
            expires_at_unix: 100,
            sources: Vec::new(),
        };
        quote.quote_id = solvent_core::crosschain::leg_quote_id(&quote);
        store.insert(&quote).await.unwrap();
        assert_eq!(store.load(quote.quote_id).await.unwrap(), Some(quote));
    }
}
