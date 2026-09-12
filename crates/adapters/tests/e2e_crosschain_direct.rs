//! Hermetic direct-route cross-chain E2E: two authenticated chain services behind the keyless
//! proxy, with real HTTP clients, routers, core services, and SQLite state. Execution is a
//! deterministic test double at the chain boundary; Solidity effects stay in the contract E2Es.

use std::collections::BTreeMap;
use std::sync::Arc;

use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{HeaderValue, Request, StatusCode};
use axum::Router;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use solvent_adapters::crosschain::{
    SolventClient, SqliteLegQuoteStore, SqlitePreparationStore, SqliteSagaStore, SqliteStepStore,
};
use solvent_adapters::http::{crosschain_internal_router, crosschain_proxy_router};
use solvent_adapters::ledger::SqliteLedgerStore;
use solvent_core::crosschain::{CrossChainProxy, LocalCrossChainService, LocalStepService};
use solvent_core::deps::crosschain::{
    LegQuoter, LegQuoterError, PreparationStore, RemoteSolvent, SagaStore, SagaStoreError,
    StepStore, StepStoreError, StepValidator, StepValidatorError,
};
use solvent_core::deps::execution::{Execution, ExecutionError, SimError, SimGate};
use solvent_core::deps::ledger::{BudgetSource, BudgetSourceError, Clock};
use solvent_core::ledger::LedgerService;
use solvent_core::primitives::crosschain::{
    AggregateQuote, ChainExecutionPlan, CrossChainLifecycleStage, CrossChainRoute, CrossChainSaga,
    LegQuote, LegQuoteRequest, LegRole, PreparationState, PreparedStep, RemoteCommand, SagaState,
    StepValidationContext,
};
use solvent_core::primitives::execution::{
    ExecHandle, ExecStatus, FillTx, SimVerdict, TrackedFill,
};
use solvent_core::primitives::ledger::{AccountKey, ReservationSource};
use solvent_core::primitives::{
    AggregateQuoteId, ChainId, CrossChainOrderId, CrossChainStepId, IntentId, MakerId,
    PrepareToken, ReservationId, StrategyHash,
};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;
use tokio::sync::Mutex;
use tower::ServiceExt;

const NOW: u64 = 1_700_000_000;
const INTERNAL_TOKEN: &str = "e2e-private-token";
const ORIGIN_CHAIN: ChainId = ChainId(11_155_111);
const DESTINATION_CHAIN: ChainId = ChainId(84_532);

#[derive(Clone)]
struct FixedClock;

impl Clock for FixedClock {
    fn now_unix(&self) -> u64 {
        NOW
    }
}

struct AbundantBudget;

#[async_trait]
impl BudgetSource for AbundantBudget {
    async fn budget(&self, _: &AccountKey) -> Result<U256, BudgetSourceError> {
        Ok(U256::MAX)
    }
}

struct ScenarioQuoter {
    chain_id: ChainId,
}

#[async_trait]
impl LegQuoter for ScenarioQuoter {
    async fn quote(&self, request: &LegQuoteRequest) -> Result<LegQuote, LegQuoterError> {
        if request.local_chain != self.chain_id {
            return Err(LegQuoterError::Backend("wrong chain".to_string()));
        }
        let amount_out = match request.role {
            LegRole::Origin => request.amount,
            LegRole::Destination => request.amount - U256::from(7),
        };
        let sources = match request.role {
            LegRole::Origin => Vec::new(),
            LegRole::Destination => vec![ReservationSource {
                maker: MakerId(Address::from([self.chain_id.0 as u8; 20])),
                strategy_hash: StrategyHash(B256::from([0x44; 32])),
                token: request.output_token,
                amount: amount_out,
            }],
        };
        let mut quote = LegQuote {
            quote_id: B256::ZERO,
            request_id: request.request_id,
            role: request.role,
            local_chain: request.local_chain,
            remote_chain: request.remote_chain,
            input_token: request.input_token,
            output_token: request.output_token,
            amount_in: request.amount,
            amount_out,
            route: request.route,
            price_impact_bps: None,
            block_number: self.chain_id.0,
            expires_at_unix: request.deadline_unix.min(NOW + 300),
            sources,
        };
        quote.quote_id = solvent_core::crosschain::leg_quote_id(&quote);
        Ok(quote)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Submission {
    chain_id: u64,
    intent: IntentId,
    reservation: ReservationId,
    target: Address,
    value: U256,
    calldata: Bytes,
}

struct DeterministicExecution {
    chain_id: ChainId,
    fills: Mutex<BTreeMap<B256, (FillTx, ReservationId, ExecStatus)>>,
    submissions: Arc<Mutex<Vec<Submission>>>,
}

impl DeterministicExecution {
    fn new(chain_id: ChainId, submissions: Arc<Mutex<Vec<Submission>>>) -> Self {
        Self {
            chain_id,
            fills: Mutex::new(BTreeMap::new()),
            submissions,
        }
    }
}

#[async_trait]
impl Execution for DeterministicExecution {
    async fn submit(
        &self,
        fill: &FillTx,
        reservation: ReservationId,
    ) -> Result<ExecHandle, ExecutionError> {
        let handle = ExecHandle(fill.intent.0);
        let mut fills = self.fills.lock().await;
        if fills.contains_key(&handle.0) {
            return Ok(handle);
        }
        self.submissions.lock().await.push(Submission {
            chain_id: self.chain_id.0,
            intent: fill.intent,
            reservation,
            target: fill.filler,
            value: fill.value,
            calldata: fill.calldata.clone(),
        });
        fills.insert(handle.0, (fill.clone(), reservation, ExecStatus::Pending));
        Ok(handle)
    }

    async fn status(&self, handle: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError> {
        Ok(self
            .fills
            .lock()
            .await
            .get(&handle.0)
            .map(|(_, _, status)| status.clone()))
    }

    async fn forget(&self, intent: IntentId) -> Result<(), ExecutionError> {
        self.fills.lock().await.remove(&intent.0);
        Ok(())
    }

    async fn tracked(&self) -> Result<Vec<TrackedFill>, ExecutionError> {
        Ok(self
            .fills
            .lock()
            .await
            .values()
            .map(|(fill, reservation, _)| TrackedFill::new(fill.intent, *reservation))
            .collect())
    }

    async fn tick(&self) -> Result<(), ExecutionError> {
        for (id, (_, _, status)) in self.fills.lock().await.iter_mut() {
            if matches!(status, ExecStatus::Pending) {
                *status = ExecStatus::Confirmed {
                    block: self.chain_id.0 * 1_000 + 1,
                    tx: *id,
                };
            }
        }
        Ok(())
    }
}

struct AcceptSimulation;

#[async_trait]
impl SimGate for AcceptSimulation {
    async fn simulate(&self, _: &FillTx) -> Result<SimVerdict, SimError> {
        Ok(SimVerdict::Ok)
    }
}

struct ScenarioStepValidator;

#[async_trait]
impl StepValidator for ScenarioStepValidator {
    async fn validate(
        &self,
        _: &StepValidationContext,
        _: &PreparedStep,
    ) -> Result<(), StepValidatorError> {
        Ok(())
    }
}

struct RecordingSagaStore {
    inner: Arc<SqliteSagaStore>,
    history: Arc<Mutex<Vec<SagaState>>>,
}

#[async_trait]
impl SagaStore for RecordingSagaStore {
    async fn insert(&self, saga: &CrossChainSaga) -> Result<CrossChainSaga, SagaStoreError> {
        let stored = self.inner.insert(saga).await?;
        self.history.lock().await.push(stored.state);
        Ok(stored)
    }

    async fn load(
        &self,
        order_id: CrossChainOrderId,
    ) -> Result<Option<CrossChainSaga>, SagaStoreError> {
        self.inner.load(order_id).await
    }

    async fn update(&self, saga: &CrossChainSaga) -> Result<(), SagaStoreError> {
        self.inner.update(saga).await?;
        self.history.lock().await.push(saga.state);
        Ok(())
    }

    async fn recoverable(&self) -> Result<Vec<CrossChainSaga>, SagaStoreError> {
        self.inner.recoverable().await
    }
}

struct ChainHarness {
    chain_id: ChainId,
    base_url: String,
    pool: SqlitePool,
    preparations: Arc<SqlitePreparationStore>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for ChainHarness {
    fn drop(&mut self) {
        self.server.abort();
    }
}

struct Scenario {
    proxy: Arc<CrossChainProxy>,
    router: Router,
    origin: ChainHarness,
    destination: ChainHarness,
    saga_store: Arc<SqliteSagaStore>,
    state_history: Arc<Mutex<Vec<SagaState>>>,
    submissions: Arc<Mutex<Vec<Submission>>>,
}

impl Scenario {
    async fn new() -> Self {
        let submissions = Arc::new(Mutex::new(Vec::new()));
        let origin = chain_service(ORIGIN_CHAIN, Arc::clone(&submissions)).await;
        let destination = chain_service(DESTINATION_CHAIN, Arc::clone(&submissions)).await;

        let pool = memory_pool().await;
        let saga_store = Arc::new(SqliteSagaStore::new(pool));
        saga_store.migrate().await.expect("migrate proxy store");
        let state_history = Arc::new(Mutex::new(Vec::new()));
        let recording: Arc<dyn SagaStore> = Arc::new(RecordingSagaStore {
            inner: Arc::clone(&saga_store),
            history: Arc::clone(&state_history),
        });
        let origin_remote: Arc<dyn RemoteSolvent> = Arc::new(
            SolventClient::from_base_url(&origin.base_url, INTERNAL_TOKEN).expect("origin client"),
        );
        let destination_remote: Arc<dyn RemoteSolvent> = Arc::new(
            SolventClient::from_base_url(&destination.base_url, INTERNAL_TOKEN)
                .expect("destination client"),
        );
        let proxy = Arc::new(CrossChainProxy::new(
            origin_remote,
            destination_remote,
            recording,
        ));
        let router = crosschain_proxy_router(Arc::clone(&proxy), Arc::new(FixedClock));
        Self {
            proxy,
            router,
            origin,
            destination,
            saga_store,
            state_history,
            submissions,
        }
    }

    async fn quote(&self, request_byte: u8) -> AggregateQuote {
        let body = serde_json::json!({
            "request_id": B256::from([request_byte; 32]),
            "origin_chain_id": ORIGIN_CHAIN.0,
            "destination_chain_id": DESTINATION_CHAIN.0,
            "origin_token_in": Address::from([0xa1; 20]),
            "origin_token_out": Address::from([0xb1; 20]),
            "destination_token_in": Address::from([0xb2; 20]),
            "destination_token_out": Address::from([0xa2; 20]),
            "amount_in": U256::from(1_000),
            "destination_amount_in": U256::from(900),
            "deadline_unix": NOW + 600,
            "route": CrossChainRoute::Direct,
        });
        post_json(&self.router, "/v1/cross-chain/quote", body).await
    }

    async fn start(&self, order_id: CrossChainOrderId, quote: &AggregateQuote) -> CrossChainSaga {
        let body = serde_json::json!({
            "order_id": order_id,
            "quote": quote,
            "origin_plan": plan(quote, LegRole::Origin),
            "destination_plan": plan(quote, LegRole::Destination),
        });
        let response: OrderEnvelope = post_json(&self.router, "/v1/cross-chain/orders", body).await;
        response.order
    }

    async fn order(&self, order_id: CrossChainOrderId) -> CrossChainSaga {
        let response: OrderEnvelope =
            get_json(&self.router, &format!("/v1/cross-chain/orders/{order_id}")).await;
        response.order
    }
}

#[derive(Deserialize)]
struct OrderEnvelope {
    order: CrossChainSaga,
}

async fn memory_pool() -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open SQLite")
}

async fn chain_service(
    chain_id: ChainId,
    submissions: Arc<Mutex<Vec<Submission>>>,
) -> ChainHarness {
    let pool = memory_pool().await;
    let preparations = Arc::new(SqlitePreparationStore::new(pool.clone()));
    preparations.migrate().await.expect("migrate chain store");
    let quotes = Arc::new(SqliteLegQuoteStore::new(pool.clone()));
    let ledger_store = Arc::new(SqliteLedgerStore::new(pool.clone()));
    let ledger = Arc::new(LedgerService::new(
        ledger_store,
        Arc::new(AbundantBudget),
        Arc::new(FixedClock),
    ));
    let local = Arc::new(LocalCrossChainService::new(
        chain_id,
        Arc::new(ScenarioQuoter { chain_id }),
        quotes,
        preparations.clone(),
        ledger,
        Arc::new(FixedClock),
    ));

    let targets = targets(chain_id);
    let steps = Arc::new(
        LocalStepService::new(
            chain_id,
            Address::from([chain_id.0 as u8; 20]),
            targets,
            Arc::new(SqliteStepStore::new(pool.clone())),
            Arc::new(DeterministicExecution::new(chain_id, submissions)),
            Arc::new(AcceptSimulation),
        )
        .with_validator(Arc::new(ScenarioStepValidator)),
    );
    let mut authorization = HeaderValue::from_static("Bearer e2e-private-token");
    authorization.set_sensitive(true);
    let router = crosschain_internal_router(local, steps, authorization);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind chain service");
    let address = listener.local_addr().expect("chain service address");
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("serve chain service");
    });
    ChainHarness {
        chain_id,
        base_url: format!("http://{address}/"),
        pool,
        preparations,
        server,
    }
}

fn targets(chain_id: ChainId) -> BTreeMap<RemoteCommand, Address> {
    let commands: &[RemoteCommand] = if chain_id == ORIGIN_CHAIN {
        &[RemoteCommand::ClaimOrigin, RemoteCommand::DispatchRepayment]
    } else {
        &[
            RemoteCommand::Deliver,
            RemoteCommand::DispatchFillProof,
            RemoteCommand::CloseDestination,
        ]
    };
    commands
        .iter()
        .copied()
        .map(|command| (command, target(chain_id, command)))
        .collect()
}

fn target(chain_id: ChainId, command: RemoteCommand) -> Address {
    Address::from([(chain_id.0 as u8).wrapping_add(command as u8); 20])
}

fn plan(quote: &AggregateQuote, role: LegRole) -> ChainExecutionPlan {
    let chain_id = match role {
        LegRole::Origin => quote.origin.local_chain,
        LegRole::Destination => quote.destination.local_chain,
    };
    let commands: &[RemoteCommand] = match role {
        LegRole::Origin => &[RemoteCommand::ClaimOrigin, RemoteCommand::DispatchRepayment],
        LegRole::Destination => &[
            RemoteCommand::Deliver,
            RemoteCommand::DispatchFillProof,
            RemoteCommand::CloseDestination,
        ],
    };
    ChainExecutionPlan {
        aggregate_id: quote.id,
        chain_id,
        steps: commands
            .iter()
            .copied()
            .map(|command| PreparedStep {
                command,
                target: target(chain_id, command),
                value: U256::from(command as u8),
                calldata: Bytes::from(vec![chain_id.0 as u8, command as u8]),
            })
            .collect(),
    }
}

fn step_id(
    order_id: CrossChainOrderId,
    chain_id: ChainId,
    command: RemoteCommand,
) -> CrossChainStepId {
    let mut bytes = Vec::with_capacity(41);
    bytes.extend_from_slice(order_id.0.as_slice());
    bytes.extend_from_slice(&chain_id.0.to_be_bytes());
    bytes.push(command as u8);
    CrossChainStepId(keccak256(bytes))
}

async fn post_json<T: DeserializeOwned>(
    router: &Router,
    path: &str,
    value: serde_json::Value,
) -> T {
    let response = router
        .clone()
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&value).expect("serialize body"),
                ))
                .expect("request"),
        )
        .await
        .expect("proxy response");
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body"),
    )
    .expect("deserialize response")
}

async fn get_json<T: DeserializeOwned>(router: &Router, path: &str) -> T {
    let response = router
        .clone()
        .oneshot(Request::get(path).body(Body::empty()).expect("request"))
        .await
        .expect("proxy response");
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body"),
    )
    .expect("deserialize response")
}

async fn ledger_state(pool: &SqlitePool, token: PrepareToken) -> (String, Option<String>) {
    sqlx::query_as("SELECT state, filled FROM ledger_reservation WHERE id = ?")
        .bind(token.0.to_vec())
        .fetch_one(pool)
        .await
        .expect("ledger row")
}

async fn assert_preparation(chain: &ChainHarness, token: PrepareToken, state: PreparationState) {
    let preparation = chain
        .preparations
        .load(token)
        .await
        .expect("load preparation")
        .expect("stored preparation");
    assert_eq!(preparation.chain_id, chain.chain_id);
    assert_eq!(preparation.state, state);
}

#[tokio::test]
async fn staged_aggregate_binding_survives_sqlite_reopen() {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let database = directory.path().join("crosschain.sqlite");
    let url = format!("sqlite://{}?mode=rwc", database.display());
    let order_id = CrossChainOrderId(B256::from([0x41; 32]));
    let aggregate_id = AggregateQuoteId(B256::from([0x42; 32]));
    let expected = PreparedStep {
        command: RemoteCommand::Deliver,
        target: target(DESTINATION_CHAIN, RemoteCommand::Deliver),
        value: U256::ZERO,
        calldata: Bytes::from_static(b"durable-step"),
    };
    let plan = ChainExecutionPlan {
        aggregate_id,
        chain_id: DESTINATION_CHAIN,
        steps: vec![expected.clone()],
    };

    let pool = SqlitePool::connect(&url).await.expect("open SQLite");
    SqlitePreparationStore::new(pool.clone())
        .migrate()
        .await
        .expect("migrate");
    SqliteStepStore::new(pool.clone())
        .stage(order_id, &plan)
        .await
        .expect("stage plan");
    pool.close().await;

    let reopened = SqlitePool::connect(&url).await.expect("reopen SQLite");
    let store = SqliteStepStore::new(reopened.clone());
    assert_eq!(
        store
            .aggregate_id(order_id, RemoteCommand::Deliver)
            .await
            .expect("load aggregate binding"),
        Some(aggregate_id)
    );
    assert_eq!(
        store
            .load(order_id, RemoteCommand::Deliver)
            .await
            .expect("load staged step"),
        Some(expected)
    );
    store
        .stage(order_id, &plan)
        .await
        .expect("same order and aggregate retry");
    let second_order = CrossChainOrderId(B256::from([0x43; 32]));
    assert!(matches!(
        store.stage(second_order, &plan).await,
        Err(StepStoreError::Conflict)
    ));
    let second_steps: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM crosschain_step WHERE order_id = ?")
            .bind(second_order.0.to_vec())
            .fetch_one(&reopened)
            .await
            .expect("count second-order steps");
    assert_eq!(second_steps, 0);
    reopened.close().await;
}

#[tokio::test]
async fn direct_route_traverses_every_durable_backend_stage() {
    let scenario = Scenario::new().await;
    let quote = scenario.quote(0x31).await;
    assert_eq!(quote.amount_in, U256::from(1_000));
    assert_eq!(quote.amount_out, U256::from(893));
    assert_eq!(quote.origin.local_chain, ORIGIN_CHAIN);
    assert_eq!(quote.destination.local_chain, DESTINATION_CHAIN);
    assert_eq!(quote.expires_at_unix, NOW + 300);

    let order_id = CrossChainOrderId(B256::from([0x51; 32]));
    let mut saga = scenario.start(order_id, &quote).await;
    assert_eq!(saga.state, SagaState::Prepared);
    let origin_token = saga.origin_prepare.expect("origin preparation");
    let destination_token = saga.destination_prepare.expect("destination preparation");
    assert_preparation(&scenario.origin, origin_token, PreparationState::Committed).await;
    assert_preparation(
        &scenario.destination,
        destination_token,
        PreparationState::Committed,
    )
    .await;
    assert_eq!(
        ledger_state(&scenario.origin.pool, origin_token).await.0,
        "committed"
    );
    assert_eq!(
        ledger_state(&scenario.destination.pool, destination_token)
            .await
            .0,
        "committed"
    );
    let origin_steps: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM crosschain_step")
        .fetch_one(&scenario.origin.pool)
        .await
        .expect("origin staged steps");
    let destination_steps: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM crosschain_step")
        .fetch_one(&scenario.destination.pool)
        .await
        .expect("destination staged steps");
    assert_eq!((origin_steps, destination_steps), (2, 3));
    assert_eq!(scenario.order(order_id).await, saga);
    assert_eq!(
        scenario
            .saga_store
            .load(order_id)
            .await
            .expect("load saga")
            .expect("stored saga"),
        saga
    );

    let expected_states = [
        SagaState::DestinationFinalized,
        SagaState::OriginPending,
        SagaState::OriginFinalized,
        SagaState::RepaymentPending,
        SagaState::Complete,
    ];
    let expected_commands = [
        (DESTINATION_CHAIN, RemoteCommand::Deliver),
        (DESTINATION_CHAIN, RemoteCommand::DispatchFillProof),
        (ORIGIN_CHAIN, RemoteCommand::ClaimOrigin),
        (ORIGIN_CHAIN, RemoteCommand::DispatchRepayment),
        (DESTINATION_CHAIN, RemoteCommand::CloseDestination),
    ];

    for (index, (expected_state, (chain_id, command))) in expected_states
        .iter()
        .zip(expected_commands.iter())
        .enumerate()
    {
        saga = scenario
            .proxy
            .advance(order_id, NOW + index as u64 + 1)
            .await
            .expect("advance saga");
        assert_eq!(saga.state, *expected_state, "advance {index}");
        assert_eq!(scenario.order(order_id).await, saga);

        let submissions = scenario.submissions.lock().await;
        assert_eq!(submissions.len(), index + 1);
        let submitted = &submissions[index];
        let expected_id = step_id(order_id, *chain_id, *command);
        assert_eq!(submitted.chain_id, chain_id.0);
        assert_eq!(submitted.intent, IntentId(expected_id.0));
        assert_eq!(submitted.reservation, ReservationId(expected_id.0));
        assert_eq!(submitted.target, target(*chain_id, *command));
        assert_eq!(submitted.value, U256::from(*command as u8));
        assert_eq!(
            submitted.calldata,
            Bytes::from(vec![chain_id.0 as u8, *command as u8])
        );
        drop(submissions);

        let evidence = match command {
            RemoteCommand::Deliver => saga.destination.as_ref(),
            RemoteCommand::DispatchFillProof => saga.fill_proof.as_ref(),
            RemoteCommand::ClaimOrigin => saga.origin.as_ref(),
            RemoteCommand::DispatchRepayment => saga.repayment.as_ref(),
            RemoteCommand::CloseDestination => None,
        };
        if let Some(evidence) = evidence {
            assert_eq!(evidence.command_id, expected_id);
            assert_eq!(evidence.transaction_hash, Some(expected_id.0));
            assert_eq!(evidence.block_number, Some(chain_id.0 * 1_000 + 1));
        }
    }

    assert_preparation(&scenario.origin, origin_token, PreparationState::Executed).await;
    assert_preparation(
        &scenario.destination,
        destination_token,
        PreparationState::Executed,
    )
    .await;
    let origin_ledger = ledger_state(&scenario.origin.pool, origin_token).await;
    let destination_ledger = ledger_state(&scenario.destination.pool, destination_token).await;
    assert_eq!(
        origin_ledger,
        ("posted".to_string(), Some("[]".to_string()))
    );
    assert_eq!(destination_ledger.0, "posted");
    assert!(destination_ledger.1.is_some());

    assert_eq!(
        *scenario.state_history.lock().await,
        vec![
            SagaState::Quoted,
            SagaState::Preparing,
            SagaState::Preparing,
            SagaState::Preparing,
            SagaState::Prepared,
            SagaState::DestinationFinalized,
            SagaState::OriginPending,
            SagaState::OriginFinalized,
            SagaState::RepaymentPending,
            SagaState::Complete,
        ]
    );
    assert_eq!(
        saga.lifecycle
            .iter()
            .map(|event| (event.stage, event.at))
            .collect::<Vec<_>>(),
        vec![
            (CrossChainLifecycleStage::Quoted, NOW),
            (CrossChainLifecycleStage::DestinationFill, NOW + 1),
            (CrossChainLifecycleStage::ProofRelay, NOW + 2),
            (CrossChainLifecycleStage::OriginClaim, NOW + 3),
            (CrossChainLifecycleStage::Repayment, NOW + 4),
            (CrossChainLifecycleStage::Complete, NOW + 5),
        ]
    );
    assert!(scenario
        .saga_store
        .recoverable()
        .await
        .expect("recoverable")
        .is_empty());

    let replay = scenario
        .proxy
        .advance(order_id, NOW + 6)
        .await
        .expect("replay complete");
    assert_eq!(replay, saga);
    assert_eq!(scenario.submissions.lock().await.len(), 5);
}

#[tokio::test]
async fn step_rejects_a_preparation_from_another_order() {
    let scenario = Scenario::new().await;
    let first_quote = scenario.quote(0x61).await;
    let second_quote = scenario.quote(0x62).await;
    let first_order = CrossChainOrderId(B256::from([0x71; 32]));
    let second_order = CrossChainOrderId(B256::from([0x72; 32]));
    let first = scenario.start(first_order, &first_quote).await;
    let second = scenario.start(second_order, &second_quote).await;
    let first_token = first.destination_prepare.expect("first destination token");
    let substituted_token = second
        .destination_prepare
        .expect("second destination token");
    let command = RemoteCommand::Deliver;

    let response = reqwest::Client::new()
        .post(format!(
            "{}internal/v1/cross-chain/steps",
            scenario.destination.base_url
        ))
        .bearer_auth(INTERNAL_TOKEN)
        .json(&serde_json::json!({
            "order_id": first_order,
            "command_id": step_id(first_order, DESTINATION_CHAIN, command),
            "command": command,
            "preparation": substituted_token,
        }))
        .send()
        .await
        .expect("send substituted preparation");
    let first_state = scenario
        .destination
        .preparations
        .load(first_token)
        .await
        .expect("load first preparation")
        .expect("first preparation")
        .state;
    let substituted_state = scenario
        .destination
        .preparations
        .load(substituted_token)
        .await
        .expect("load substituted preparation")
        .expect("substituted preparation")
        .state;

    assert_eq!(
        (response.status(), first_state, substituted_state),
        (
            StatusCode::BAD_REQUEST,
            PreparationState::Committed,
            PreparationState::Committed,
        ),
        "a step must be bound to the aggregate quote that created its preparation"
    );
    assert!(scenario.submissions.lock().await.is_empty());
}

#[tokio::test]
async fn aggregate_quote_cannot_be_staged_under_a_second_order() {
    let scenario = Scenario::new().await;
    let quote = scenario.quote(0x81).await;
    let first_order = CrossChainOrderId(B256::from([0x82; 32]));
    let second_order = CrossChainOrderId(B256::from([0x83; 32]));
    let first = scenario.start(first_order, &quote).await;
    let origin_token = first.origin_prepare.expect("origin preparation");
    let destination_token = first.destination_prepare.expect("destination preparation");

    let body = serde_json::json!({
        "order_id": second_order,
        "quote": quote,
        "origin_plan": plan(&quote, LegRole::Origin),
        "destination_plan": plan(&quote, LegRole::Destination),
    });
    let response = scenario
        .router
        .clone()
        .oneshot(
            Request::post("/v1/cross-chain/orders")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&body).expect("serialize body"),
                ))
                .expect("request"),
        )
        .await
        .expect("proxy response");

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert!(scenario
        .saga_store
        .load(second_order)
        .await
        .expect("load second saga")
        .is_none());
    assert_preparation(&scenario.origin, origin_token, PreparationState::Committed).await;
    assert_preparation(
        &scenario.destination,
        destination_token,
        PreparationState::Committed,
    )
    .await;
    assert!(scenario.submissions.lock().await.is_empty());
}
