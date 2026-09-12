use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode};
use solvent_adapters::crosschain::{
    AlloyStepValidator, SqliteLegQuoteStore, SqlitePreparationStore, SqliteStepStore,
};
use solvent_adapters::http::crosschain_internal_router;
use solvent_adapters::ledger::SqliteLedgerStore;
use solvent_core::crosschain::{
    aggregate_quote, leg_quote_id, LocalCrossChainService, LocalStepService,
};
use solvent_core::deps::crosschain::{LegQuoter, LegQuoterError, StepStore};
use solvent_core::deps::execution::{Execution, ExecutionError, SimError, SimGate};
use solvent_core::deps::ledger::{BudgetSource, BudgetSourceError, Clock};
use solvent_core::ledger::LedgerService;
use solvent_core::primitives::crosschain::{
    ChainExecutionPlan, CrossChainRoute, LegQuote, LegQuoteRequest, LegRole, PreparedStep,
    RemoteCommand, StepValidationContext,
};
use solvent_core::primitives::execution::{
    ExecHandle, ExecStatus, FillTx, SimVerdict, TrackedFill,
};
use solvent_core::primitives::ledger::AccountKey;
use solvent_core::primitives::{ChainId, CrossChainOrderId, IntentId, ReservationId};
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

sol! {
    function confirmDirectRepayment(bytes32 orderId, bytes repaymentProof);
    function completeRepayment(bytes32 orderId, bytes message, bytes attestation);
}

const CHAIN_ID: ChainId = ChainId(42_161);
const NOW: u64 = 1_800_000_000;

struct FixedClock;

impl Clock for FixedClock {
    fn now_unix(&self) -> u64 {
        NOW
    }
}

struct DeterministicQuoter;

#[async_trait]
impl LegQuoter for DeterministicQuoter {
    async fn quote(&self, request: &LegQuoteRequest) -> Result<LegQuote, LegQuoterError> {
        let mut quote = LegQuote {
            quote_id: B256::ZERO,
            request_id: request.request_id,
            role: request.role,
            local_chain: request.local_chain,
            remote_chain: request.remote_chain,
            input_token: request.input_token,
            output_token: request.output_token,
            amount_in: request.amount,
            amount_out: U256::from(850),
            route: request.route,
            block_number: 100,
            expires_at_unix: NOW + 60,
            sources: Vec::new(),
        };
        quote.quote_id = leg_quote_id(&quote);
        Ok(quote)
    }
}

struct UnusedBudget;

#[async_trait]
impl BudgetSource for UnusedBudget {
    async fn budget(&self, _account: &AccountKey) -> Result<U256, BudgetSourceError> {
        Ok(U256::ZERO)
    }
}

#[derive(Default)]
struct ChainActionProbe {
    simulations: AtomicUsize,
    submissions: AtomicUsize,
}

#[async_trait]
impl SimGate for ChainActionProbe {
    async fn simulate(&self, _fill: &FillTx) -> Result<SimVerdict, SimError> {
        self.simulations.fetch_add(1, Ordering::SeqCst);
        Ok(SimVerdict::Ok)
    }
}

#[async_trait]
impl Execution for ChainActionProbe {
    async fn submit(
        &self,
        _fill: &FillTx,
        _reservation: ReservationId,
    ) -> Result<ExecHandle, ExecutionError> {
        self.submissions.fetch_add(1, Ordering::SeqCst);
        Ok(ExecHandle(B256::ZERO))
    }

    async fn status(&self, _handle: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError> {
        Ok(None)
    }

    async fn forget(&self, _intent: IntentId) -> Result<(), ExecutionError> {
        Ok(())
    }

    async fn tracked(&self) -> Result<Vec<TrackedFill>, ExecutionError> {
        Ok(Vec::new())
    }

    async fn tick(&self) -> Result<(), ExecutionError> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum InvalidStage {
    DifferentOrder,
    WrongSelector,
    NativeValue,
    TamperedLocalQuote,
}

async fn assert_private_stage_rejected(case: InvalidStage) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open isolated sqlite database");
    let preparations = Arc::new(SqlitePreparationStore::new(pool.clone()));
    preparations.migrate().await.expect("apply schema");

    let clock: Arc<dyn Clock> = Arc::new(FixedClock);
    let ledger = Arc::new(LedgerService::new(
        Arc::new(SqliteLedgerStore::new(pool.clone())),
        Arc::new(UnusedBudget),
        Arc::clone(&clock),
    ));
    let local = Arc::new(LocalCrossChainService::new(
        CHAIN_ID,
        Arc::new(DeterministicQuoter),
        Arc::new(SqliteLegQuoteStore::new(pool.clone())),
        preparations,
        ledger,
        clock,
    ));

    let intended_order = CrossChainOrderId(B256::repeat_byte(0x11));
    let different_order = CrossChainOrderId(B256::repeat_byte(0x22));
    let destination_app = Address::repeat_byte(0x44);
    let destination_request = LegQuoteRequest {
        request_id: B256::repeat_byte(0x66),
        role: LegRole::Destination,
        local_chain: CHAIN_ID,
        remote_chain: ChainId(1),
        input_token: Address::repeat_byte(0x71),
        output_token: Address::repeat_byte(0x72),
        amount: U256::from(900),
        deadline_unix: NOW + 60,
        route: CrossChainRoute::Direct,
    };
    let destination_quote = local
        .quote(&destination_request)
        .await
        .expect("issue destination quote");
    let mut origin_quote = LegQuote {
        quote_id: B256::ZERO,
        request_id: destination_request.request_id,
        role: LegRole::Origin,
        local_chain: ChainId(1),
        remote_chain: CHAIN_ID,
        input_token: Address::repeat_byte(0x73),
        output_token: Address::repeat_byte(0x74),
        amount_in: U256::from(1_000),
        amount_out: U256::from(1_000),
        route: CrossChainRoute::Direct,
        block_number: 101,
        expires_at_unix: NOW + 60,
        sources: Vec::new(),
    };
    origin_quote.quote_id = leg_quote_id(&origin_quote);
    let quote = aggregate_quote(origin_quote.clone(), destination_quote.clone(), NOW)
        .expect("aggregate quote");
    let mut context = StepValidationContext {
        order_id: intended_order,
        quote,
        role: LegRole::Destination,
    };
    if matches!(case, InvalidStage::TamperedLocalQuote) {
        let mut tampered = destination_quote;
        tampered.amount_out = tampered.amount_out.saturating_add(U256::from(1));
        tampered.quote_id = leg_quote_id(&tampered);
        context.quote = aggregate_quote(origin_quote, tampered, NOW).expect("tampered aggregate");
    }
    let calldata = match case {
        InvalidStage::DifferentOrder => confirmDirectRepaymentCall {
            orderId: different_order.0,
            repaymentProof: Bytes::from_static(b"proof-for-another-order"),
        }
        .abi_encode(),
        InvalidStage::WrongSelector => completeRepaymentCall {
            orderId: intended_order.0,
            message: Bytes::new(),
            attestation: Bytes::new(),
        }
        .abi_encode(),
        InvalidStage::NativeValue | InvalidStage::TamperedLocalQuote => {
            confirmDirectRepaymentCall {
                orderId: intended_order.0,
                repaymentProof: Bytes::from_static(b"proof"),
            }
            .abi_encode()
        }
    };
    let plan = ChainExecutionPlan {
        aggregate_id: context.quote.id,
        chain_id: CHAIN_ID,
        steps: vec![PreparedStep {
            command: RemoteCommand::CloseDestination,
            target: destination_app,
            value: if matches!(case, InvalidStage::NativeValue) {
                U256::from(1)
            } else {
                U256::ZERO
            },
            calldata: calldata.into(),
        }],
    };

    let step_store = Arc::new(SqliteStepStore::new(pool));
    let probe = Arc::new(ChainActionProbe::default());
    let mut targets = BTreeMap::new();
    targets.insert(RemoteCommand::CloseDestination, destination_app);
    let steps = Arc::new(
        LocalStepService::new(
            CHAIN_ID,
            Address::repeat_byte(0x55),
            targets,
            step_store.clone(),
            probe.clone(),
            probe.clone(),
        )
        .with_validator(Arc::new(AlloyStepValidator::new(
            Address::repeat_byte(0x55),
            Address::repeat_byte(0x72),
        ))),
    );
    let authorization = HeaderValue::from_static("Bearer semantic-binding-test");
    let router = crosschain_internal_router(local, steps, authorization);
    let body = serde_json::to_vec(&serde_json::json!({
        "context": context,
        "plan": plan,
    }))
    .expect("serialize stage request");

    let response = router
        .oneshot(
            Request::post("/internal/v1/cross-chain/stage")
                .header("authorization", "Bearer semantic-binding-test")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("build stage request"),
        )
        .await
        .expect("serve stage request");
    let staged = step_store
        .load(intended_order, RemoteCommand::CloseDestination)
        .await
        .expect("inspect staged step");

    assert_eq!(
        (
            response.status(),
            staged,
            probe.simulations.load(Ordering::SeqCst),
            probe.submissions.load(Ordering::SeqCst),
        ),
        (StatusCode::BAD_REQUEST, None, 0, 0),
        "invalid semantic plan terms must be rejected before persistence",
    );
}

#[tokio::test]
async fn private_stage_rejects_calldata_bound_to_a_different_order() {
    assert_private_stage_rejected(InvalidStage::DifferentOrder).await;
}

#[tokio::test]
async fn private_stage_rejects_the_wrong_contract_selector() {
    assert_private_stage_rejected(InvalidStage::WrongSelector).await;
}

#[tokio::test]
async fn private_stage_rejects_client_supplied_native_value() {
    assert_private_stage_rejected(InvalidStage::NativeValue).await;
}

#[tokio::test]
async fn private_stage_rejects_a_quote_not_issued_by_the_local_service() {
    assert_private_stage_rejected(InvalidStage::TamperedLocalQuote).await;
}
