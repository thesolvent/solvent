use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use async_trait::async_trait;
use solvent_adapters::crosschain::{SqlitePreparationStore, SqliteStepStore};
use solvent_core::crosschain::LocalStepService;
use solvent_core::deps::crosschain::{
    StepStore, StepStoreError, StepValidator, StepValidatorError,
};
use solvent_core::deps::execution::{Execution, ExecutionError, SimError, SimGate};
use solvent_core::primitives::crosschain::{
    ChainExecutionPlan, LegRole, Preparation, PreparationState, PreparedStep, RemoteCommand,
    StepValidationContext,
};
use solvent_core::primitives::execution::{
    ExecHandle, ExecStatus, FillTx, SimVerdict, TrackedFill,
};
use solvent_core::primitives::{
    AggregateQuoteId, ChainId, CrossChainOrderId, CrossChainStepId, IntentId, PrepareToken,
    ReservationId, SolventError,
};
use sqlx::SqlitePool;

#[derive(Default)]
struct ActionProbe(AtomicUsize);

struct AcceptValidator;

#[async_trait]
impl StepValidator for AcceptValidator {
    async fn validate(
        &self,
        _: &StepValidationContext,
        _: &PreparedStep,
    ) -> Result<(), StepValidatorError> {
        Ok(())
    }
}

#[async_trait]
impl Execution for ActionProbe {
    async fn submit(&self, _: &FillTx, _: ReservationId) -> Result<ExecHandle, ExecutionError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ExecHandle(B256::ZERO))
    }

    async fn status(&self, _: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }

    async fn forget(&self, _: IntentId) -> Result<(), ExecutionError> {
        Ok(())
    }

    async fn tracked(&self) -> Result<Vec<TrackedFill>, ExecutionError> {
        Ok(Vec::new())
    }

    async fn tick(&self) -> Result<(), ExecutionError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait]
impl SimGate for ActionProbe {
    async fn simulate(&self, _: &FillTx) -> Result<SimVerdict, SimError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(SimVerdict::Ok)
    }
}

#[tokio::test]
async fn same_aggregate_preparation_with_wrong_role_is_rejected_before_execution() {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("open SQLite");
    SqlitePreparationStore::new(pool.clone())
        .migrate()
        .await
        .expect("migrate SQLite");
    let store = Arc::new(SqliteStepStore::new(pool));
    let chain_id = ChainId(42_161);
    let order_id = CrossChainOrderId(B256::repeat_byte(0x51));
    let aggregate_id = AggregateQuoteId(B256::repeat_byte(0x52));
    let command = RemoteCommand::CloseDestination;
    let target = Address::repeat_byte(0x53);
    store
        .stage(
            order_id,
            &ChainExecutionPlan {
                aggregate_id,
                chain_id,
                steps: vec![PreparedStep {
                    command,
                    target,
                    value: U256::ZERO,
                    calldata: Bytes::from_static(b"wrong-role-probe"),
                }],
            },
        )
        .await
        .expect("stage verifier plan");

    let probe = Arc::new(ActionProbe::default());
    let service = LocalStepService::new(
        chain_id,
        Address::repeat_byte(0x54),
        BTreeMap::from([(command, target)]),
        store,
        probe.clone(),
        probe.clone(),
    )
    .with_validator(Arc::new(AcceptValidator));
    let preparation = Preparation {
        token: PrepareToken(B256::repeat_byte(0x55)),
        quote_id: aggregate_id,
        role: LegRole::Origin,
        chain_id,
        state: PreparationState::Executed,
        expires_at_unix: u64::MAX,
    };
    let mut bytes = Vec::with_capacity(41);
    bytes.extend_from_slice(order_id.0.as_slice());
    bytes.extend_from_slice(&chain_id.0.to_be_bytes());
    bytes.push(command as u8);
    let command_id = CrossChainStepId(keccak256(bytes));

    let result = service
        .command(order_id, command_id, command, &preparation)
        .await;
    assert!(matches!(result, Err(SolventError::InvalidCrossChain(_))));
    assert_eq!(probe.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn same_aggregate_preparation_cannot_authorize_a_second_order() {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("open SQLite");
    SqlitePreparationStore::new(pool.clone())
        .migrate()
        .await
        .expect("migrate SQLite");
    let store = Arc::new(SqliteStepStore::new(pool));
    let chain_id = ChainId(42_161);
    let first_order = CrossChainOrderId(B256::repeat_byte(0x61));
    let second_order = CrossChainOrderId(B256::repeat_byte(0x62));
    let aggregate_id = AggregateQuoteId(B256::repeat_byte(0x63));
    let command = RemoteCommand::Deliver;
    let target = Address::repeat_byte(0x64);
    let make_plan = || ChainExecutionPlan {
        aggregate_id,
        chain_id,
        steps: vec![PreparedStep {
            command,
            target,
            value: U256::ZERO,
            calldata: Bytes::from_static(b"same-quote-second-order"),
        }],
    };
    store
        .stage(first_order, &make_plan())
        .await
        .expect("stage first order");
    assert!(matches!(
        store.stage(second_order, &make_plan()).await,
        Err(StepStoreError::Conflict)
    ));

    let probe = Arc::new(ActionProbe::default());
    let service = LocalStepService::new(
        chain_id,
        Address::repeat_byte(0x65),
        BTreeMap::from([(command, target)]),
        store,
        probe.clone(),
        probe.clone(),
    )
    .with_validator(Arc::new(AcceptValidator));
    let preparation = Preparation {
        token: PrepareToken(B256::repeat_byte(0x66)),
        quote_id: aggregate_id,
        role: LegRole::Destination,
        chain_id,
        state: PreparationState::Executed,
        expires_at_unix: u64::MAX,
    };
    let mut bytes = Vec::with_capacity(41);
    bytes.extend_from_slice(second_order.0.as_slice());
    bytes.extend_from_slice(&chain_id.0.to_be_bytes());
    bytes.push(command as u8);
    let command_id = CrossChainStepId(keccak256(bytes));

    let result = service
        .command(second_order, command_id, command, &preparation)
        .await;
    assert!(
        matches!(result, Err(SolventError::InvalidCrossChain(_))),
        "a preparation already used by one order must not authorize a second order"
    );
    assert_eq!(probe.0.load(Ordering::SeqCst), 0);
}
