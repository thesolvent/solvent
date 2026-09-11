//! Hermetic fault-injection coverage for the direct cross-chain coordinator.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use alloy::primitives::{Address, Bytes, Uint, B256, U256};
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolCall, SolStruct, SolValue};
use async_trait::async_trait;
use axum::http::HeaderValue;
use solvent_adapters::crosschain::{
    AlloyStepValidator, SolventClient, SqliteLegQuoteStore, SqlitePreparationStore,
    SqliteSagaStore, SqliteStepStore,
};
use solvent_adapters::http::crosschain_internal_router;
use solvent_adapters::ledger::SqliteLedgerStore;
use solvent_core::crosschain::{CrossChainProxy, LocalCrossChainService, LocalStepService};
use solvent_core::deps::crosschain::{
    LegQuoter, LegQuoterError, PreparationStore, RemoteProgress, RemoteSolvent, RemoteSolventError,
    SagaStore,
};
use solvent_core::deps::execution::{Execution, ExecutionError, SimError, SimGate};
use solvent_core::deps::ledger::{BudgetSource, BudgetSourceError, Clock};
use solvent_core::ledger::LedgerService;
use solvent_core::primitives::crosschain::{
    AggregateQuote, ChainExecutionPlan, CrossChainRoute, LegQuote, LegQuoteRequest, LegRole,
    Preparation, PreparationState, PreparedStep, RemoteCommand, SagaState, StepValidationContext,
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

type U48 = Uint<48, 1>;

const NOW: u64 = 1_800_000_000;
const TOKEN: &str = "failure-e2e-token";
const ORIGIN: ChainId = ChainId(1);
const DESTINATION: ChainId = ChainId(42_161);
const INPUT: Address = Address::repeat_byte(0x11);
const OUTPUT: Address = Address::repeat_byte(0x12);
const OPERATOR: Address = Address::repeat_byte(0x13);
const MAKER: Address = Address::repeat_byte(0x14);
const STRATEGY: B256 = B256::repeat_byte(0x15);
const WRAPPED_NATIVE: Address = Address::repeat_byte(0x16);

sol! {
    struct SolventCrossChainOrder {
        address user; uint256 nonce; uint256 originChainId; address originSettler;
        address compact; uint256 compactId; uint256 compactExpires; address inputToken;
        uint256 inputAmount; uint256 destinationChainId; address outputToken;
        uint256 minimumOutputAmount; address recipient; address destinationSettler;
        address fillProofVerifier; address exclusiveFiller; uint48 exclusivityEnds;
        uint48 fillDeadline; uint8 routeKind;
    }
    struct SolventMandate {
        bytes32 orderId; uint256 destinationChainId; address destinationSettler;
        address fillProofVerifier; address outputToken; uint256 minimumOutputAmount;
        address recipient; uint48 fillDeadline; address exclusiveFiller; uint8 routeKind;
    }
    struct DirectMakerQuote {
        bytes32 orderId; address maker; bytes32 destinationStrategyHash;
        bytes32 originStrategyHash; uint256 outputAmount; uint256 repaymentAmount;
        uint256 nonce; uint48 expires;
    }
    struct Component { uint256 claimant; uint256 amount; }
    struct Claim {
        bytes allocatorData; bytes sponsorSignature; address sponsor; uint256 nonce;
        uint256 expires; bytes32 witness; string witnessTypestring; uint256 id;
        uint256 allocatedAmount; Component[] claimants;
    }
    struct ProofEnvelope { uint8 version; uint8 kind; bytes payload; }
    struct ProofIdMaterial { uint256 chainId; address application; bytes32 orderId; uint8 kind; }
    struct VerifiedFill {
        bytes32 orderId; uint8 routeKind; uint256 destinationChainId; address destinationApp;
        address recipient; address outputToken; uint256 outputAmount; address destinationMaker;
        bytes32 destinationStrategyHash; bytes32 originStrategyHash; address repaymentToken;
        uint256 repaymentAmount; uint256 maxCctpFee; bytes32 makerQuoteHash; bytes32 fillId;
    }
    struct VerifiedRepayment {
        bytes32 orderId; uint256 originChainId; address originSettler; address maker;
        address repaymentToken; uint256 repaymentAmount; bytes32 repaymentId;
    }
    interface DestinationApp {
        function fillDirect(SolventCrossChainOrder order, SolventMandate mandate,
            DirectMakerQuote quote, bytes makerSignature) external;
        function confirmDirectRepayment(bytes32 orderId, bytes repaymentProof) external;
    }
    interface OriginSettler {
        function settleDirect(SolventCrossChainOrder order, SolventMandate mandate,
            bytes fillProof, Claim compactClaim) external;
    }
    interface Outbox {
        function dispatch(bytes32 orderId, bytes envelope) external payable returns (bytes32);
    }
}

struct FixedClock;

impl Clock for FixedClock {
    fn now_unix(&self) -> u64 {
        NOW
    }
}

struct UnlimitedBudget;

#[async_trait]
impl BudgetSource for UnlimitedBudget {
    async fn budget(&self, _: &AccountKey) -> Result<U256, BudgetSourceError> {
        Ok(U256::MAX)
    }
}

struct FixedQuoter {
    chain: ChainId,
}

#[async_trait]
impl LegQuoter for FixedQuoter {
    async fn quote(&self, request: &LegQuoteRequest) -> Result<LegQuote, LegQuoterError> {
        let amount_out = match request.role {
            LegRole::Origin => request.amount,
            LegRole::Destination => request.amount - U256::from(7),
        };
        let sources = match request.role {
            LegRole::Origin => Vec::new(),
            LegRole::Destination => vec![ReservationSource {
                maker: MakerId(MAKER),
                strategy_hash: StrategyHash(STRATEGY),
                token: request.output_token,
                amount: amount_out,
            }],
        };
        let mut quote = LegQuote {
            quote_id: B256::ZERO,
            request_id: request.request_id,
            role: request.role,
            local_chain: self.chain,
            remote_chain: request.remote_chain,
            input_token: request.input_token,
            output_token: request.output_token,
            amount_in: request.amount,
            amount_out,
            route: request.route,
            block_number: self.chain.0,
            expires_at_unix: NOW + 300,
            sources,
        };
        quote.quote_id = solvent_core::crosschain::leg_quote_id(&quote);
        Ok(quote)
    }
}

#[derive(Clone, Copy)]
enum TickMode {
    Confirm,
    PendingOnce,
    Fail,
}

struct ControlledExecution {
    chain: ChainId,
    fills: Mutex<BTreeMap<B256, (FillTx, ReservationId, ExecStatus, usize)>>,
    submissions: AtomicUsize,
    modes: Mutex<BTreeMap<Address, TickMode>>,
}

impl ControlledExecution {
    fn new(chain: ChainId) -> Self {
        Self {
            chain,
            fills: Mutex::new(BTreeMap::new()),
            submissions: AtomicUsize::new(0),
            modes: Mutex::new(BTreeMap::new()),
        }
    }
    async fn mode(&self, command: RemoteCommand, mode: TickMode) {
        self.modes
            .lock()
            .await
            .insert(target(self.chain, command), mode);
    }
    fn count(&self) -> usize {
        self.submissions.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Execution for ControlledExecution {
    async fn submit(
        &self,
        fill: &FillTx,
        reservation: ReservationId,
    ) -> Result<ExecHandle, ExecutionError> {
        let handle = ExecHandle(fill.intent.0);
        let mut fills = self.fills.lock().await;
        fills.entry(handle.0).or_insert_with(|| {
            self.submissions.fetch_add(1, Ordering::SeqCst);
            (fill.clone(), reservation, ExecStatus::Pending, 0)
        });
        Ok(handle)
    }
    async fn status(&self, handle: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError> {
        Ok(self
            .fills
            .lock()
            .await
            .get(&handle.0)
            .map(|entry| entry.2.clone()))
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
            .map(|(fill, reservation, _, _)| TrackedFill::new(fill.intent, *reservation))
            .collect())
    }
    async fn tick(&self) -> Result<(), ExecutionError> {
        let modes = self.modes.lock().await.clone();
        for (id, (fill, _, status, polls)) in self.fills.lock().await.iter_mut() {
            if !matches!(status, ExecStatus::Pending) {
                continue;
            }
            *polls += 1;
            match modes
                .get(&fill.filler)
                .copied()
                .unwrap_or(TickMode::Confirm)
            {
                TickMode::Confirm => {
                    *status = ExecStatus::Confirmed {
                        block: self.chain.0 * 1000 + 1,
                        tx: *id,
                    }
                }
                TickMode::PendingOnce if *polls > 1 => {
                    *status = ExecStatus::Confirmed {
                        block: self.chain.0 * 1000 + 1,
                        tx: *id,
                    }
                }
                TickMode::PendingOnce => {}
                TickMode::Fail => {
                    *status = ExecStatus::Failed {
                        reason: "injected execution failure".to_string(),
                    }
                }
            }
        }
        Ok(())
    }
}

struct ControlledSimulation {
    rejected: Mutex<BTreeSet<Address>>,
    calls: AtomicUsize,
}

impl ControlledSimulation {
    fn new() -> Self {
        Self {
            rejected: Mutex::new(BTreeSet::new()),
            calls: AtomicUsize::new(0),
        }
    }
    async fn reject(&self, chain: ChainId, command: RemoteCommand) {
        self.rejected.lock().await.insert(target(chain, command));
    }
}

#[async_trait]
impl SimGate for ControlledSimulation {
    async fn simulate(&self, fill: &FillTx) -> Result<SimVerdict, SimError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.rejected.lock().await.contains(&fill.filler) {
            Ok(SimVerdict::Reject {
                reason: "injected simulation rejection".to_string(),
            })
        } else {
            Ok(SimVerdict::Ok)
        }
    }
}

#[derive(Default)]
struct RemoteFaults {
    reject_prepare: AtomicBool,
    fail_commit: AtomicBool,
    fail_commands: Mutex<BTreeSet<RemoteCommand>>,
}

struct FaultRemote {
    inner: SolventClient,
    faults: Arc<RemoteFaults>,
}

#[async_trait]
impl RemoteSolvent for FaultRemote {
    async fn quote(&self, request: &LegQuoteRequest) -> Result<LegQuote, RemoteSolventError> {
        self.inner.quote(request).await
    }
    async fn stage(
        &self,
        context: &StepValidationContext,
        plan: &ChainExecutionPlan,
    ) -> Result<(), RemoteSolventError> {
        self.inner.stage(context, plan).await
    }
    async fn prepare(
        &self,
        aggregate_id: AggregateQuoteId,
        quote: &LegQuote,
    ) -> Result<Preparation, RemoteSolventError> {
        if self.faults.reject_prepare.swap(false, Ordering::SeqCst) {
            Err(RemoteSolventError::Rejected(
                "injected prepare rejection".to_string(),
            ))
        } else {
            self.inner.prepare(aggregate_id, quote).await
        }
    }
    async fn commit(&self, token: PrepareToken) -> Result<Preparation, RemoteSolventError> {
        if self.faults.fail_commit.swap(false, Ordering::SeqCst) {
            Err(RemoteSolventError::Unavailable(
                "injected transient commit failure".to_string(),
            ))
        } else {
            self.inner.commit(token).await
        }
    }
    async fn inspect(&self, token: PrepareToken) -> Result<Preparation, RemoteSolventError> {
        self.inner.inspect(token).await
    }
    async fn release(&self, token: PrepareToken) -> Result<Preparation, RemoteSolventError> {
        self.inner.release(token).await
    }
    async fn command(
        &self,
        order_id: CrossChainOrderId,
        command_id: CrossChainStepId,
        command: RemoteCommand,
        preparation: PrepareToken,
    ) -> Result<RemoteProgress, RemoteSolventError> {
        if self.faults.fail_commands.lock().await.remove(&command) {
            Ok(RemoteProgress::Failed {
                reason: format!("injected {command:?} failure"),
            })
        } else {
            self.inner
                .command(order_id, command_id, command, preparation)
                .await
        }
    }
}

struct ChainHarness {
    base_url: String,
    pool: SqlitePool,
    preparations: Arc<SqlitePreparationStore>,
    execution: Arc<ControlledExecution>,
    simulation: Arc<ControlledSimulation>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for ChainHarness {
    fn drop(&mut self) {
        self.server.abort();
    }
}

struct Scenario {
    proxy: Arc<CrossChainProxy>,
    sagas: Arc<SqliteSagaStore>,
    origin: ChainHarness,
    destination: ChainHarness,
    origin_faults: Arc<RemoteFaults>,
    destination_faults: Arc<RemoteFaults>,
}

impl Scenario {
    async fn new() -> Self {
        let origin = chain_service(ORIGIN).await;
        let destination = chain_service(DESTINATION).await;
        let origin_faults = Arc::new(RemoteFaults::default());
        let destination_faults = Arc::new(RemoteFaults::default());
        let origin_remote: Arc<dyn RemoteSolvent> = Arc::new(FaultRemote {
            inner: SolventClient::from_base_url(&origin.base_url, TOKEN).expect("origin client"),
            faults: Arc::clone(&origin_faults),
        });
        let destination_remote: Arc<dyn RemoteSolvent> = Arc::new(FaultRemote {
            inner: SolventClient::from_base_url(&destination.base_url, TOKEN)
                .expect("destination client"),
            faults: Arc::clone(&destination_faults),
        });
        let pool = memory_pool().await;
        let sagas = Arc::new(SqliteSagaStore::new(pool));
        sagas.migrate().await.expect("migrate saga store");
        let proxy = Arc::new(CrossChainProxy::new(
            origin_remote,
            destination_remote,
            sagas.clone(),
        ));
        Self {
            proxy,
            sagas,
            origin,
            destination,
            origin_faults,
            destination_faults,
        }
    }

    async fn quote(&self, byte: u8) -> AggregateQuote {
        self.proxy
            .quote(
                &request(
                    byte,
                    LegRole::Origin,
                    ORIGIN,
                    DESTINATION,
                    INPUT,
                    OUTPUT,
                    U256::from(1_000),
                ),
                &request(
                    byte,
                    LegRole::Destination,
                    DESTINATION,
                    ORIGIN,
                    INPUT,
                    OUTPUT,
                    U256::from(900),
                ),
                NOW,
            )
            .await
            .expect("aggregate quote")
    }

    async fn start(
        &self,
        byte: u8,
    ) -> (
        CrossChainOrderId,
        Result<
            solvent_core::primitives::crosschain::CrossChainSaga,
            solvent_core::primitives::SolventError,
        >,
    ) {
        let quote = self.quote(byte).await;
        let (order_id, origin_plan, destination_plan) = plans(&quote, byte);
        let result = self
            .proxy
            .start(order_id, quote, &origin_plan, &destination_plan, NOW)
            .await;
        (order_id, result)
    }

    async fn stored(
        &self,
        order: CrossChainOrderId,
    ) -> solvent_core::primitives::crosschain::CrossChainSaga {
        self.sagas
            .load(order)
            .await
            .expect("load saga")
            .expect("stored saga")
    }
}

async fn chain_service(chain: ChainId) -> ChainHarness {
    let pool = memory_pool().await;
    let preparations = Arc::new(SqlitePreparationStore::new(pool.clone()));
    preparations.migrate().await.expect("migrate chain store");
    let ledger = Arc::new(LedgerService::new(
        Arc::new(SqliteLedgerStore::new(pool.clone())),
        Arc::new(UnlimitedBudget),
        Arc::new(FixedClock),
    ));
    let local = Arc::new(LocalCrossChainService::new(
        chain,
        Arc::new(FixedQuoter { chain }),
        Arc::new(SqliteLegQuoteStore::new(pool.clone())),
        preparations.clone(),
        ledger,
        Arc::new(FixedClock),
    ));
    let execution = Arc::new(ControlledExecution::new(chain));
    let simulation = Arc::new(ControlledSimulation::new());
    let steps = Arc::new(
        LocalStepService::new(
            chain,
            OPERATOR,
            targets(chain),
            Arc::new(SqliteStepStore::new(pool.clone())),
            execution.clone(),
            simulation.clone(),
        )
        .with_validator(Arc::new(
            AlloyStepValidator::new(OPERATOR, WRAPPED_NATIVE).with_applications(
                target(ORIGIN, RemoteCommand::ClaimOrigin),
                target(DESTINATION, RemoteCommand::Deliver),
            ),
        )),
    );
    let mut authorization = HeaderValue::from_static("Bearer failure-e2e-token");
    authorization.set_sensitive(true);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind chain service");
    let address = listener.local_addr().expect("chain address");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            crosschain_internal_router(local, steps, authorization),
        )
        .await
        .expect("serve chain service");
    });
    ChainHarness {
        base_url: format!("http://{address}/"),
        pool,
        preparations,
        execution,
        simulation,
        server,
    }
}

async fn memory_pool() -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open SQLite")
}

fn request(
    byte: u8,
    role: LegRole,
    local: ChainId,
    remote: ChainId,
    input: Address,
    output: Address,
    amount: U256,
) -> LegQuoteRequest {
    LegQuoteRequest {
        request_id: B256::repeat_byte(byte),
        role,
        local_chain: local,
        remote_chain: remote,
        input_token: input,
        output_token: output,
        amount,
        route: CrossChainRoute::Direct,
        deadline_unix: NOW + 600,
    }
}

fn target(chain: ChainId, command: RemoteCommand) -> Address {
    Address::repeat_byte((chain.0 as u8).wrapping_add(command as u8))
}

fn targets(chain: ChainId) -> BTreeMap<RemoteCommand, Address> {
    let commands: &[RemoteCommand] = if chain == ORIGIN {
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
        .map(|command| (command, target(chain, command)))
        .collect()
}

fn plans(
    quote: &AggregateQuote,
    byte: u8,
) -> (CrossChainOrderId, ChainExecutionPlan, ChainExecutionPlan) {
    let order = SolventCrossChainOrder {
        user: Address::repeat_byte(byte),
        nonce: U256::from(byte),
        originChainId: U256::from(ORIGIN.0),
        originSettler: target(ORIGIN, RemoteCommand::ClaimOrigin),
        compact: Address::repeat_byte(0x21),
        compactId: U256::from(1),
        compactExpires: U256::from(NOW + 600),
        inputToken: INPUT,
        inputAmount: quote.amount_in,
        destinationChainId: U256::from(DESTINATION.0),
        outputToken: OUTPUT,
        minimumOutputAmount: quote.amount_out,
        recipient: Address::repeat_byte(0x22),
        destinationSettler: target(DESTINATION, RemoteCommand::Deliver),
        fillProofVerifier: Address::repeat_byte(0x23),
        exclusiveFiller: OPERATOR,
        exclusivityEnds: U48::from(NOW + 100),
        fillDeadline: U48::from(NOW + 500),
        routeKind: 1,
    };
    let order_id = CrossChainOrderId(order.eip712_hash_struct());
    let mandate = SolventMandate {
        orderId: order_id.0,
        destinationChainId: order.destinationChainId,
        destinationSettler: order.destinationSettler,
        fillProofVerifier: order.fillProofVerifier,
        outputToken: order.outputToken,
        minimumOutputAmount: order.minimumOutputAmount,
        recipient: order.recipient,
        fillDeadline: order.fillDeadline,
        exclusiveFiller: order.exclusiveFiller,
        routeKind: order.routeKind,
    };
    let maker_quote = DirectMakerQuote {
        orderId: order_id.0,
        maker: MAKER,
        destinationStrategyHash: STRATEGY,
        originStrategyHash: B256::repeat_byte(0x24),
        outputAmount: quote.amount_out,
        repaymentAmount: quote.amount_in,
        nonce: U256::from(1),
        expires: U48::from(NOW + 200),
    };
    let deliver = DestinationApp::fillDirectCall {
        order: order.clone(),
        mandate: mandate.clone(),
        quote: maker_quote.clone(),
        makerSignature: Bytes::from_static(b"signature"),
    }
    .abi_encode()
    .into();
    let claim = OriginSettler::settleDirectCall {
        order: order.clone(),
        mandate,
        fillProof: fill_id(order_id).abi_encode().into(),
        compactClaim: Claim {
            allocatorData: Bytes::new(),
            sponsorSignature: Bytes::new(),
            sponsor: Address::repeat_byte(0x25),
            nonce: U256::ZERO,
            expires: U256::from(NOW + 500),
            witness: B256::ZERO,
            witnessTypestring: String::new(),
            id: U256::ZERO,
            allocatedAmount: quote.amount_in,
            claimants: Vec::new(),
        },
    }
    .abi_encode()
    .into();
    let fill = VerifiedFill {
        orderId: order_id.0,
        routeKind: 1,
        destinationChainId: U256::from(DESTINATION.0),
        destinationApp: order.destinationSettler,
        recipient: order.recipient,
        outputToken: quote.destination.output_token,
        outputAmount: quote.amount_out,
        destinationMaker: MAKER,
        destinationStrategyHash: STRATEGY,
        originStrategyHash: maker_quote.originStrategyHash,
        repaymentToken: quote.origin.input_token,
        repaymentAmount: quote.amount_in,
        maxCctpFee: U256::ZERO,
        makerQuoteHash: maker_quote.eip712_signing_hash(&eip712_domain! {
            name: "Solvent Cross-Chain Aqua App",
            version: "1",
            chain_id: DESTINATION.0,
            verifying_contract: order.destinationSettler,
        }),
        fillId: fill_id(order_id),
    };
    let repayment = VerifiedRepayment {
        orderId: order_id.0,
        originChainId: U256::from(ORIGIN.0),
        originSettler: order.originSettler,
        maker: MAKER,
        repaymentToken: quote.origin.input_token,
        repaymentAmount: quote.amount_in,
        repaymentId: repayment_id(order_id),
    };
    let dispatch = |command| PreparedStep {
        command,
        target: target(
            if command == RemoteCommand::DispatchFillProof {
                DESTINATION
            } else {
                ORIGIN
            },
            command,
        ),
        value: U256::ZERO,
        calldata: Outbox::dispatchCall {
            orderId: order_id.0,
            envelope: ProofEnvelope {
                version: 1,
                kind: if command == RemoteCommand::DispatchFillProof {
                    0
                } else {
                    1
                },
                payload: if command == RemoteCommand::DispatchFillProof {
                    fill.abi_encode().into()
                } else {
                    repayment.abi_encode().into()
                },
            }
            .abi_encode()
            .into(),
        }
        .abi_encode()
        .into(),
    };
    let origin_plan = ChainExecutionPlan {
        aggregate_id: quote.id,
        chain_id: ORIGIN,
        steps: vec![
            PreparedStep {
                command: RemoteCommand::ClaimOrigin,
                target: target(ORIGIN, RemoteCommand::ClaimOrigin),
                value: U256::ZERO,
                calldata: claim,
            },
            dispatch(RemoteCommand::DispatchRepayment),
        ],
    };
    let destination_plan = ChainExecutionPlan {
        aggregate_id: quote.id,
        chain_id: DESTINATION,
        steps: vec![
            PreparedStep {
                command: RemoteCommand::Deliver,
                target: target(DESTINATION, RemoteCommand::Deliver),
                value: U256::ZERO,
                calldata: deliver,
            },
            dispatch(RemoteCommand::DispatchFillProof),
            PreparedStep {
                command: RemoteCommand::CloseDestination,
                target: target(DESTINATION, RemoteCommand::CloseDestination),
                value: U256::ZERO,
                calldata: DestinationApp::confirmDirectRepaymentCall {
                    orderId: order_id.0,
                    repaymentProof: repayment_id(order_id).abi_encode().into(),
                }
                .abi_encode()
                .into(),
            },
        ],
    };
    (order_id, origin_plan, destination_plan)
}

fn fill_id(order_id: CrossChainOrderId) -> B256 {
    proof_id(
        DESTINATION,
        target(DESTINATION, RemoteCommand::Deliver),
        order_id,
        0,
    )
}

fn repayment_id(order_id: CrossChainOrderId) -> B256 {
    proof_id(
        ORIGIN,
        target(ORIGIN, RemoteCommand::ClaimOrigin),
        order_id,
        1,
    )
}

fn proof_id(chain: ChainId, application: Address, order_id: CrossChainOrderId, kind: u8) -> B256 {
    alloy::primitives::keccak256(
        ProofIdMaterial {
            chainId: U256::from(chain.0),
            application,
            orderId: order_id.0,
            kind,
        }
        .abi_encode(),
    )
}

async fn prep_count(chain: &ChainHarness) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM crosschain_preparation")
        .fetch_one(&chain.pool)
        .await
        .expect("count preparations")
}
async fn step_count(chain: &ChainHarness) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM crosschain_step")
        .fetch_one(&chain.pool)
        .await
        .expect("count steps")
}
async fn ledger_state(chain: &ChainHarness, token: PrepareToken) -> (String, Option<String>) {
    sqlx::query_as("SELECT state, filled FROM ledger_reservation WHERE id = ?")
        .bind(token.0.to_vec())
        .fetch_one(&chain.pool)
        .await
        .expect("ledger row")
}
async fn assert_prep(chain: &ChainHarness, token: PrepareToken, state: PreparationState) {
    assert_eq!(
        chain
            .preparations
            .load(token)
            .await
            .expect("load preparation")
            .expect("preparation")
            .state,
        state
    );
}
async fn finish(scenario: &Scenario, order: CrossChainOrderId) {
    for _ in 0..12 {
        if scenario
            .proxy
            .advance(order, NOW + 1)
            .await
            .expect("advance")
            .state
            == SagaState::Complete
        {
            return;
        }
    }
    panic!("saga did not complete");
}

#[tokio::test]
async fn prepare_rejections_compensate_first_and_second_leg() {
    let first = Scenario::new().await;
    first
        .origin_faults
        .reject_prepare
        .store(true, Ordering::SeqCst);
    let (order, result) = first.start(0x31).await;
    assert_eq!(
        result.expect("terminal preparation result").state,
        SagaState::FailedBeforeDelivery
    );
    assert_eq!(
        (
            prep_count(&first.origin).await,
            prep_count(&first.destination).await
        ),
        (0, 0)
    );
    assert_eq!(
        (
            step_count(&first.origin).await,
            step_count(&first.destination).await
        ),
        (2, 3)
    );
    assert_eq!(
        first.stored(order).await.state,
        SagaState::FailedBeforeDelivery
    );

    let second = Scenario::new().await;
    second
        .destination_faults
        .reject_prepare
        .store(true, Ordering::SeqCst);
    let (_, result) = second.start(0x32).await;
    let saga = result.expect("terminal preparation result");
    assert_eq!(saga.state, SagaState::FailedBeforeDelivery);
    let origin = saga.origin_prepare.expect("origin preparation");
    assert_prep(&second.origin, origin, PreparationState::Released).await;
    assert_eq!(ledger_state(&second.origin, origin).await.0, "voided");
    assert_eq!(prep_count(&second.destination).await, 0);
}

#[tokio::test]
async fn transient_commit_failure_resumes_from_durable_preparations() {
    let scenario = Scenario::new().await;
    scenario
        .destination_faults
        .fail_commit
        .store(true, Ordering::SeqCst);
    let (order, result) = scenario.start(0x41).await;
    assert!(result.is_err());
    let preparing = scenario.stored(order).await;
    assert_eq!(preparing.state, SagaState::Preparing);
    let origin = preparing.origin_prepare.expect("origin preparation");
    let destination = preparing
        .destination_prepare
        .expect("destination preparation");
    assert_prep(&scenario.origin, origin, PreparationState::Committed).await;
    assert_prep(
        &scenario.destination,
        destination,
        PreparationState::Prepared,
    )
    .await;
    assert_eq!(
        scenario
            .proxy
            .advance(order, NOW + 1)
            .await
            .expect("resume commit")
            .state,
        SagaState::Prepared
    );
    assert_prep(
        &scenario.destination,
        destination,
        PreparationState::Committed,
    )
    .await;
}

#[tokio::test]
async fn deliver_simulation_rejection_never_submits_or_posts_capital() {
    let scenario = Scenario::new().await;
    scenario
        .destination
        .simulation
        .reject(DESTINATION, RemoteCommand::Deliver)
        .await;
    let (order, started) = scenario.start(0x51).await;
    let started = started.expect("start");
    let origin = started.origin_prepare.expect("origin preparation");
    let destination = started
        .destination_prepare
        .expect("destination preparation");
    let failed = scenario
        .proxy
        .advance(order, NOW + 1)
        .await
        .expect("reject delivery");
    assert_eq!(failed.state, SagaState::FailedBeforeDelivery);
    assert_eq!(scenario.destination.execution.count(), 0);
    assert_eq!(
        scenario.destination.simulation.calls.load(Ordering::SeqCst),
        1
    );
    assert_prep(&scenario.origin, origin, PreparationState::Released).await;
    assert_prep(
        &scenario.destination,
        destination,
        PreparationState::Released,
    )
    .await;
    assert_eq!(
        ledger_state(&scenario.origin, origin).await,
        ("voided".to_string(), None)
    );
    assert_eq!(
        ledger_state(&scenario.destination, destination).await,
        ("voided".to_string(), None)
    );
}

#[tokio::test]
async fn delivery_can_remain_pending_then_finalize_and_can_fail_before_delivery() {
    let pending = Scenario::new().await;
    pending
        .destination
        .execution
        .mode(RemoteCommand::Deliver, TickMode::PendingOnce)
        .await;
    let (order, started) = pending.start(0x61).await;
    assert_eq!(started.expect("start").state, SagaState::Prepared);
    assert_eq!(
        pending
            .proxy
            .advance(order, NOW + 1)
            .await
            .expect("pending delivery")
            .state,
        SagaState::DestinationPending
    );
    assert_eq!(pending.destination.execution.count(), 1);
    assert_eq!(
        pending
            .proxy
            .advance(order, NOW + 1)
            .await
            .expect("finalized delivery")
            .state,
        SagaState::DestinationFinalized
    );
    assert_eq!(pending.destination.execution.count(), 1);

    let failed = Scenario::new().await;
    failed
        .destination
        .execution
        .mode(RemoteCommand::Deliver, TickMode::Fail)
        .await;
    let (order, started) = failed.start(0x62).await;
    let started = started.expect("start");
    let destination = started
        .destination_prepare
        .expect("destination preparation");
    assert_eq!(
        failed
            .proxy
            .advance(order, NOW + 1)
            .await
            .expect("failed delivery")
            .state,
        SagaState::FailedBeforeDelivery
    );
    assert_prep(&failed.destination, destination, PreparationState::Released).await;
    assert_eq!(failed.destination.execution.count(), 1);
}

#[tokio::test]
async fn post_delivery_proof_and_repayment_failures_reconcile_without_releasing_capital() {
    let scenario = Scenario::new().await;
    scenario
        .destination_faults
        .fail_commands
        .lock()
        .await
        .insert(RemoteCommand::DispatchFillProof);
    scenario
        .origin_faults
        .fail_commands
        .lock()
        .await
        .insert(RemoteCommand::DispatchRepayment);
    let (order, started) = scenario.start(0x71).await;
    let started = started.expect("start");
    let origin = started.origin_prepare.expect("origin preparation");
    let destination = started
        .destination_prepare
        .expect("destination preparation");
    assert_eq!(
        scenario
            .proxy
            .advance(order, NOW + 1)
            .await
            .expect("deliver")
            .state,
        SagaState::DestinationFinalized
    );
    assert_prep(
        &scenario.destination,
        destination,
        PreparationState::Executed,
    )
    .await;
    assert_eq!(
        scenario
            .proxy
            .advance(order, NOW + 1)
            .await
            .expect("proof failure")
            .state,
        SagaState::NeedsReconcile
    );
    assert_eq!(
        ledger_state(&scenario.destination, destination).await.0,
        "posted"
    );
    assert_eq!(
        scenario
            .proxy
            .advance(order, NOW + 1)
            .await
            .expect("proof retry")
            .state,
        SagaState::OriginPending
    );
    assert_eq!(
        scenario
            .proxy
            .advance(order, NOW + 1)
            .await
            .expect("claim")
            .state,
        SagaState::OriginFinalized
    );
    assert_prep(&scenario.origin, origin, PreparationState::Executed).await;
    assert_eq!(
        scenario
            .proxy
            .advance(order, NOW + 1)
            .await
            .expect("repayment failure")
            .state,
        SagaState::NeedsReconcile
    );
    assert_eq!(ledger_state(&scenario.origin, origin).await.0, "posted");
    assert_eq!(
        ledger_state(&scenario.destination, destination).await.0,
        "posted"
    );
    finish(&scenario, order).await;
    assert_eq!(scenario.stored(order).await.state, SagaState::Complete);
    assert_eq!(
        scenario.origin.execution.count() + scenario.destination.execution.count(),
        5
    );
}

#[tokio::test]
async fn duplicate_concurrent_advances_submit_each_deterministic_command_once() {
    let scenario = Scenario::new().await;
    let (order, started) = scenario.start(0x81).await;
    assert_eq!(started.expect("start").state, SagaState::Prepared);
    let (left, right) = tokio::join!(
        scenario.proxy.advance(order, NOW + 1),
        scenario.proxy.advance(order, NOW + 1)
    );
    assert_eq!(
        left.expect("left advance").state,
        SagaState::DestinationFinalized
    );
    assert_eq!(
        right.expect("right advance").state,
        SagaState::DestinationFinalized
    );
    assert_eq!(scenario.destination.execution.count(), 1);
    finish(&scenario, order).await;
    assert_eq!(
        scenario.origin.execution.count() + scenario.destination.execution.count(),
        5
    );
    assert_eq!(scenario.stored(order).await.state, SagaState::Complete);
}
