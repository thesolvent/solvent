//! Restart recovery across every durable cross-chain saga boundary.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use alloy::primitives::{keccak256, Address, Bytes, Uint, B256, U256};
use alloy::{
    sol,
    sol_types::{eip712_domain, SolCall, SolStruct, SolValue},
};
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
    CctpCompletion, CctpCompletionError, CctpPreparedStep, CctpQuoteTerms, LegQuoter,
    LegQuoterError, RemoteProgress, RemoteSolvent, RemoteSolventError,
};
use solvent_core::deps::execution::{Execution, ExecutionError, SimError, SimGate};
use solvent_core::deps::ledger::{BudgetSource, BudgetSourceError, Clock};
use solvent_core::ledger::LedgerService;
use solvent_core::primitives::crosschain::{
    AggregateQuote, ChainExecutionPlan, CrossChainRoute, CrossChainSaga, LegQuote, LegQuoteRequest,
    LegRole, Preparation, PreparedStep, RemoteCommand, SagaState, StepValidationContext,
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

const NOW: u64 = 1_700_000_000;
const ORIGIN: ChainId = ChainId(11_155_111);
const DESTINATION: ChainId = ChainId(84_532);
const TOKEN: &str = "restart-private";
type U48 = Uint<48, 1>;

sol! {
    struct SolventCrossChainOrder { address user; uint256 nonce; uint256 originChainId; address originSettler; address compact; uint256 compactId; uint256 compactExpires; address inputToken; uint256 inputAmount; uint256 destinationChainId; address outputToken; uint256 minimumOutputAmount; address recipient; address destinationSettler; address fillProofVerifier; address exclusiveFiller; uint48 exclusivityEnds; uint48 fillDeadline; uint8 routeKind; }
    struct SolventMandate { bytes32 orderId; uint256 destinationChainId; address destinationSettler; address fillProofVerifier; address outputToken; uint256 minimumOutputAmount; address recipient; uint48 fillDeadline; address exclusiveFiller; uint8 routeKind; }
    struct DirectMakerQuote { bytes32 orderId; address maker; bytes32 destinationStrategyHash; bytes32 originStrategyHash; uint256 outputAmount; uint256 repaymentAmount; uint256 nonce; uint48 expires; }
    struct MakerCreditQuote { bytes32 orderId; address maker; bytes32 destinationStrategyHash; uint256 outputAmount; uint256 usdcDue; uint256 maxCctpFee; uint256 nonce; uint48 expires; }
    struct Component { uint256 claimant; uint256 amount; }
    struct Claim { bytes allocatorData; bytes sponsorSignature; address sponsor; uint256 nonce; uint256 expires; bytes32 witness; string witnessTypestring; uint256 id; uint256 allocatedAmount; Component[] claimants; }
    struct SwapOrder { address maker; uint256 traits; bytes data; }
    struct ProofEnvelope { uint8 version; uint8 kind; bytes payload; }
    struct ProofIdMaterial { uint256 chainId; address application; bytes32 orderId; uint8 kind; }
    struct VerifiedFill { bytes32 orderId; uint8 routeKind; uint256 destinationChainId; address destinationApp; address recipient; address outputToken; uint256 outputAmount; address destinationMaker; bytes32 destinationStrategyHash; bytes32 originStrategyHash; address repaymentToken; uint256 repaymentAmount; uint256 maxCctpFee; bytes32 makerQuoteHash; bytes32 fillId; }
    struct VerifiedRepayment { bytes32 orderId; uint256 originChainId; address originSettler; address maker; address repaymentToken; uint256 repaymentAmount; bytes32 repaymentId; }
    interface DestinationApp { function fillDirect(SolventCrossChainOrder order, SolventMandate mandate, DirectMakerQuote quote, bytes signature); function fillCredit(SolventCrossChainOrder order, SolventMandate mandate, MakerCreditQuote quote, bytes signature); function confirmDirectRepayment(bytes32 orderId, bytes proof); function completeRepayment(bytes32 orderId, bytes message, bytes attestation); }
    interface OriginSettler { function settleDirect(SolventCrossChainOrder order, SolventMandate mandate, bytes proof, Claim claim); function settleRouted(SolventCrossChainOrder order, SolventMandate mandate, bytes proof, Claim claim, SwapOrder makerOrder); }
    interface ProofOutbox { function dispatch(bytes32 orderId, bytes envelope) payable returns (bytes32); }
}

#[derive(Clone)]
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

struct Quoter(ChainId);

#[async_trait]
impl LegQuoter for Quoter {
    async fn quote(&self, request: &LegQuoteRequest) -> Result<LegQuote, LegQuoterError> {
        let amount_out = match (request.role, request.route) {
            (LegRole::Destination, _) => request.amount - U256::from(7),
            (LegRole::Origin, CrossChainRoute::Cctp) => U256::from(903),
            _ => request.amount,
        };
        let sources = match (request.role, request.route) {
            (LegRole::Destination, _) => vec![ReservationSource {
                maker: MakerId(maker(self.0)),
                strategy_hash: StrategyHash(B256::repeat_byte(0x44)),
                token: request.output_token,
                amount: amount_out,
            }],
            (LegRole::Origin, CrossChainRoute::Cctp) => vec![ReservationSource {
                maker: MakerId(maker(self.0)),
                strategy_hash: StrategyHash(keccak256(swap_order(self.0).abi_encode())),
                token: request.output_token,
                amount: amount_out,
            }],
            _ => Vec::new(),
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
            block_number: self.0 .0,
            expires_at_unix: NOW + 300,
            sources,
        };
        quote.quote_id = solvent_core::crosschain::leg_quote_id(&quote);
        Ok(quote)
    }
}

struct AcceptSimulation;

#[async_trait]
impl SimGate for AcceptSimulation {
    async fn simulate(&self, _: &FillTx) -> Result<SimVerdict, SimError> {
        Ok(SimVerdict::Ok)
    }
}

struct DurableExecution {
    chain: ChainId,
    pool: SqlitePool,
    confirm_on_tick: bool,
}

impl DurableExecution {
    async fn initialize(pool: SqlitePool, chain: ChainId, confirm_on_tick: bool) -> Self {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS restart_execution (
                intent BLOB PRIMARY KEY NOT NULL CHECK(length(intent) = 32),
                reservation BLOB NOT NULL CHECK(length(reservation) = 32),
                state TEXT NOT NULL CHECK(state IN ('pending', 'confirmed')),
                attempts INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .expect("create durable execution seam");
        Self {
            chain,
            pool,
            confirm_on_tick,
        }
    }
}

#[async_trait]
impl Execution for DurableExecution {
    async fn submit(
        &self,
        fill: &FillTx,
        reservation: ReservationId,
    ) -> Result<ExecHandle, ExecutionError> {
        sqlx::query(
            "INSERT INTO restart_execution (intent, reservation, state, attempts)
             VALUES (?, ?, 'pending', 1)
             ON CONFLICT(intent) DO UPDATE SET attempts = attempts + 1",
        )
        .bind(fill.intent.0.to_vec())
        .bind(reservation.0.to_vec())
        .execute(&self.pool)
        .await
        .map_err(execution_error)?;
        Ok(ExecHandle(fill.intent.0))
    }

    async fn status(&self, handle: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError> {
        let state: Option<String> =
            sqlx::query_scalar("SELECT state FROM restart_execution WHERE intent = ?")
                .bind(handle.0.to_vec())
                .fetch_optional(&self.pool)
                .await
                .map_err(execution_error)?;
        Ok(state.map(|state| match state.as_str() {
            "confirmed" => ExecStatus::Confirmed {
                block: self.chain.0,
                tx: handle.0,
            },
            _ => ExecStatus::Pending,
        }))
    }

    async fn forget(&self, intent: IntentId) -> Result<(), ExecutionError> {
        sqlx::query("DELETE FROM restart_execution WHERE intent = ?")
            .bind(intent.0.to_vec())
            .execute(&self.pool)
            .await
            .map_err(execution_error)?;
        Ok(())
    }

    async fn tracked(&self) -> Result<Vec<TrackedFill>, ExecutionError> {
        let rows: Vec<(Vec<u8>, Vec<u8>)> = sqlx::query_as(
            "SELECT intent, reservation FROM restart_execution WHERE state = 'pending'",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(execution_error)?;
        rows.into_iter()
            .map(|(intent, reservation)| {
                let intent = B256::try_from(intent.as_slice())
                    .map_err(|error| ExecutionError::Engine(error.to_string()))?;
                let reservation = B256::try_from(reservation.as_slice())
                    .map_err(|error| ExecutionError::Engine(error.to_string()))?;
                Ok(TrackedFill::new(
                    IntentId(intent),
                    ReservationId(reservation),
                ))
            })
            .collect()
    }

    async fn tick(&self) -> Result<(), ExecutionError> {
        if self.confirm_on_tick {
            sqlx::query("UPDATE restart_execution SET state = 'confirmed' WHERE state = 'pending'")
                .execute(&self.pool)
                .await
                .map_err(execution_error)?;
        }
        Ok(())
    }
}

fn execution_error(error: impl std::fmt::Display) -> ExecutionError {
    ExecutionError::Engine(error.to_string())
}

struct OneUnavailablePrepare {
    inner: Arc<dyn RemoteSolvent>,
    unavailable: AtomicBool,
}

#[async_trait]
impl RemoteSolvent for OneUnavailablePrepare {
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
        if self.unavailable.swap(false, Ordering::SeqCst) {
            return Err(RemoteSolventError::Unavailable(
                "injected restart boundary".to_string(),
            ));
        }
        self.inner.prepare(aggregate_id, quote).await
    }

    async fn commit(&self, token: PrepareToken) -> Result<Preparation, RemoteSolventError> {
        self.inner.commit(token).await
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
        self.inner
            .command(order_id, command_id, command, preparation)
            .await
    }
}

struct DeterministicCctp {
    destination: ChainId,
    pool: SqlitePool,
    ready: bool,
}

#[async_trait]
impl CctpCompletion for DeterministicCctp {
    async fn quote_terms(&self, _: U256) -> Result<CctpQuoteTerms, CctpCompletionError> {
        Ok(CctpQuoteTerms {
            max_fee: U256::from(3),
            finality_threshold: 2_000,
        })
    }

    async fn close_step(
        &self,
        order_id: CrossChainOrderId,
        _: B256,
    ) -> Result<CctpPreparedStep, CctpCompletionError> {
        sqlx::query(
            "INSERT INTO restart_cctp_poll (order_id, attempts) VALUES (?, 1)
             ON CONFLICT(order_id) DO UPDATE SET attempts = attempts + 1",
        )
        .bind(order_id.0.to_vec())
        .execute(&self.pool)
        .await
        .map_err(|error| CctpCompletionError::Transport(error.to_string()))?;
        if !self.ready {
            return Err(CctpCompletionError::Pending);
        }
        Ok(CctpPreparedStep {
            step: PreparedStep {
                command: RemoteCommand::CloseDestination,
                target: app(self.destination),
                value: U256::ZERO,
                calldata: DestinationApp::completeRepaymentCall {
                    orderId: order_id.0,
                    message: Bytes::from_static(b"message"),
                    attestation: Bytes::from_static(b"attestation"),
                }
                .abi_encode()
                .into(),
            },
            message_id: B256::repeat_byte(0xcc),
        })
    }
}

struct DatabasePaths {
    origin: PathBuf,
    destination: PathBuf,
    proxy: PathBuf,
}

impl DatabasePaths {
    fn new(root: &Path) -> Self {
        Self {
            origin: root.join("origin.sqlite"),
            destination: root.join("destination.sqlite"),
            proxy: root.join("proxy.sqlite"),
        }
    }
}

struct ChainRuntime {
    pool: SqlitePool,
    url: String,
    server: tokio::task::JoinHandle<()>,
}

impl ChainRuntime {
    async fn shutdown(self) {
        self.server.abort();
        let _ = self.server.await;
        self.pool.close().await;
    }
}

struct Runtime {
    proxy: Arc<CrossChainProxy>,
    saga_pool: SqlitePool,
    origin: ChainRuntime,
    destination: ChainRuntime,
}

impl Runtime {
    async fn shutdown(self) {
        let Self {
            proxy,
            saga_pool,
            origin,
            destination,
        } = self;
        drop(proxy);
        origin.shutdown().await;
        destination.shutdown().await;
        saga_pool.close().await;
    }
}

async fn boot(
    paths: &DatabasePaths,
    route: CrossChainRoute,
    confirm_on_tick: bool,
    fail_destination_prepare: bool,
    cctp_ready: bool,
) -> Runtime {
    let origin = boot_chain(&paths.origin, ORIGIN, LegRole::Origin, confirm_on_tick).await;
    let destination = boot_chain(
        &paths.destination,
        DESTINATION,
        LegRole::Destination,
        confirm_on_tick,
    )
    .await;
    let saga_pool = open_pool(&paths.proxy).await;
    let sagas = Arc::new(SqliteSagaStore::new(saga_pool.clone()));
    sagas.migrate().await.expect("migrate saga database");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS restart_cctp_poll (
            order_id BLOB PRIMARY KEY NOT NULL CHECK(length(order_id) = 32),
            attempts INTEGER NOT NULL
        )",
    )
    .execute(&saga_pool)
    .await
    .expect("create CCTP poll seam");

    let origin_remote: Arc<dyn RemoteSolvent> =
        Arc::new(SolventClient::from_base_url(&origin.url, TOKEN).expect("origin client"));
    let base_destination: Arc<dyn RemoteSolvent> = Arc::new(
        SolventClient::from_base_url(&destination.url, TOKEN).expect("destination client"),
    );
    let destination_remote: Arc<dyn RemoteSolvent> = if fail_destination_prepare {
        Arc::new(OneUnavailablePrepare {
            inner: base_destination,
            unavailable: AtomicBool::new(true),
        })
    } else {
        base_destination
    };
    let mut proxy = CrossChainProxy::new(origin_remote, destination_remote, sagas);
    if route == CrossChainRoute::Cctp {
        proxy = proxy.with_cctp(Arc::new(DeterministicCctp {
            destination: DESTINATION,
            pool: saga_pool.clone(),
            ready: cctp_ready,
        }));
    }
    Runtime {
        proxy: Arc::new(proxy),
        saga_pool,
        origin,
        destination,
    }
}

async fn boot_chain(
    path: &Path,
    chain_id: ChainId,
    role: LegRole,
    confirm_on_tick: bool,
) -> ChainRuntime {
    let pool = open_pool(path).await;
    let preparations = Arc::new(SqlitePreparationStore::new(pool.clone()));
    preparations
        .migrate()
        .await
        .expect("migrate chain database");
    let ledger = Arc::new(LedgerService::new(
        Arc::new(SqliteLedgerStore::new(pool.clone())),
        Arc::new(UnlimitedBudget),
        Arc::new(FixedClock),
    ));
    ledger.recover().await.expect("recover chain ledger");
    let local = Arc::new(LocalCrossChainService::new(
        chain_id,
        Arc::new(Quoter(chain_id)),
        Arc::new(SqliteLegQuoteStore::new(pool.clone())),
        preparations,
        ledger,
        Arc::new(FixedClock),
    ));
    let execution = DurableExecution::initialize(pool.clone(), chain_id, confirm_on_tick).await;
    let steps = Arc::new(
        LocalStepService::new(
            chain_id,
            operator(chain_id),
            targets(chain_id, role),
            Arc::new(SqliteStepStore::new(pool.clone())),
            Arc::new(execution),
            Arc::new(AcceptSimulation),
        )
        .with_validator(Arc::new(
            AlloyStepValidator::new(operator(chain_id), wrapped_native(chain_id))
                .with_applications(settler(ORIGIN), app(DESTINATION)),
        )),
    );
    let router = crosschain_internal_router(
        Arc::clone(&local),
        steps,
        HeaderValue::from_static("Bearer restart-private"),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind private chain service");
    let address = listener.local_addr().expect("private service address");
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("serve private chain service");
    });
    let url = format!("http://{address}/");
    ChainRuntime { pool, url, server }
}

async fn open_pool(path: &Path) -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .expect("open file-backed SQLite")
}

fn maker(chain: ChainId) -> Address {
    Address::from([chain.0 as u8; 20])
}

fn operator(chain: ChainId) -> Address {
    Address::from([(chain.0 as u8).wrapping_add(1); 20])
}

fn app(chain: ChainId) -> Address {
    Address::from([(chain.0 as u8).wrapping_add(2); 20])
}

fn settler(chain: ChainId) -> Address {
    Address::from([(chain.0 as u8).wrapping_add(3); 20])
}

fn outbox(chain: ChainId) -> Address {
    Address::from([(chain.0 as u8).wrapping_add(4); 20])
}

fn wrapped_native(chain: ChainId) -> Address {
    Address::from([(chain.0 as u8).wrapping_add(5); 20])
}

fn targets(chain: ChainId, role: LegRole) -> BTreeMap<RemoteCommand, Address> {
    match role {
        LegRole::Origin => [
            (RemoteCommand::ClaimOrigin, settler(chain)),
            (RemoteCommand::DispatchRepayment, outbox(chain)),
        ]
        .into_iter()
        .collect(),
        LegRole::Destination => [
            (RemoteCommand::Deliver, app(chain)),
            (RemoteCommand::DispatchFillProof, outbox(chain)),
            (RemoteCommand::CloseDestination, app(chain)),
        ]
        .into_iter()
        .collect(),
    }
}

fn swap_order(chain: ChainId) -> SwapOrder {
    SwapOrder {
        maker: maker(chain),
        traits: U256::from(7),
        data: Bytes::from_static(b"restart"),
    }
}

fn claim() -> Claim {
    Claim {
        allocatorData: Bytes::new(),
        sponsorSignature: Bytes::new(),
        sponsor: Address::repeat_byte(0x91),
        nonce: U256::from(1),
        expires: U256::from(NOW + 200),
        witness: B256::repeat_byte(0x92),
        witnessTypestring: "Mandate(bytes32 witness)".into(),
        id: U256::from(2),
        allocatedAmount: U256::from(1_000),
        claimants: Vec::new(),
    }
}

fn plans(
    route: CrossChainRoute,
    quote: &AggregateQuote,
) -> (CrossChainOrderId, ChainExecutionPlan, ChainExecutionPlan) {
    let route_kind = u8::from(route == CrossChainRoute::Direct);
    let order = SolventCrossChainOrder {
        user: Address::repeat_byte(0x80),
        nonce: U256::from(route_kind + 9),
        originChainId: U256::from(ORIGIN.0),
        originSettler: settler(ORIGIN),
        compact: Address::repeat_byte(0x81),
        compactId: U256::from(2),
        compactExpires: U256::from(NOW + 200),
        inputToken: quote.origin.input_token,
        inputAmount: quote.amount_in,
        destinationChainId: U256::from(DESTINATION.0),
        outputToken: quote.destination.output_token,
        minimumOutputAmount: quote.amount_out,
        recipient: Address::repeat_byte(0x82),
        destinationSettler: app(DESTINATION),
        fillProofVerifier: Address::repeat_byte(0x83),
        exclusiveFiller: operator(DESTINATION),
        exclusivityEnds: U48::from(NOW + 50),
        fillDeadline: U48::from(NOW + 100),
        routeKind: route_kind,
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
    let domain = eip712_domain! {
        name: "Solvent Cross-Chain Aqua App",
        version: "1",
        chain_id: DESTINATION.0,
        verifying_contract: app(DESTINATION),
    };
    let (deliver, origin_strategy, maker_quote_hash, repayment_token, repayment_amount) =
        match route {
            CrossChainRoute::Direct => {
                let maker_quote = DirectMakerQuote {
                    orderId: order_id.0,
                    maker: maker(DESTINATION),
                    destinationStrategyHash: B256::repeat_byte(0x44),
                    originStrategyHash: B256::repeat_byte(0x45),
                    outputAmount: quote.amount_out,
                    repaymentAmount: quote.amount_in,
                    nonce: U256::from(3),
                    expires: U48::from(NOW + 100),
                };
                let digest = maker_quote.eip712_signing_hash(&domain);
                (
                    DestinationApp::fillDirectCall {
                        order: order.clone(),
                        mandate: mandate.clone(),
                        quote: maker_quote,
                        signature: Bytes::from_static(b"signature"),
                    }
                    .abi_encode(),
                    B256::repeat_byte(0x45),
                    digest,
                    quote.origin.input_token,
                    quote.amount_in,
                )
            }
            CrossChainRoute::Cctp => {
                let maker_quote = MakerCreditQuote {
                    orderId: order_id.0,
                    maker: maker(DESTINATION),
                    destinationStrategyHash: B256::repeat_byte(0x44),
                    outputAmount: quote.amount_out,
                    usdcDue: quote.destination.amount_in,
                    maxCctpFee: quote.bridge_fee,
                    nonce: U256::from(3),
                    expires: U48::from(NOW + 100),
                };
                let digest = maker_quote.eip712_signing_hash(&domain);
                (
                    DestinationApp::fillCreditCall {
                        order: order.clone(),
                        mandate: mandate.clone(),
                        quote: maker_quote,
                        signature: Bytes::from_static(b"signature"),
                    }
                    .abi_encode(),
                    B256::ZERO,
                    digest,
                    quote.destination.input_token,
                    quote.destination.amount_in,
                )
            }
        };
    let fill_id = proof_id(DESTINATION, app(DESTINATION), order_id, 0);
    let repayment_id = proof_id(ORIGIN, settler(ORIGIN), order_id, 1);
    let fill_envelope: Bytes = ProofEnvelope {
        version: 1,
        kind: 0,
        payload: VerifiedFill {
            orderId: order_id.0,
            routeKind: route_kind,
            destinationChainId: U256::from(DESTINATION.0),
            destinationApp: app(DESTINATION),
            recipient: order.recipient,
            outputToken: quote.destination.output_token,
            outputAmount: quote.amount_out,
            destinationMaker: maker(DESTINATION),
            destinationStrategyHash: B256::repeat_byte(0x44),
            originStrategyHash: origin_strategy,
            repaymentToken: repayment_token,
            repaymentAmount: repayment_amount,
            maxCctpFee: quote.bridge_fee,
            makerQuoteHash: maker_quote_hash,
            fillId: fill_id,
        }
        .abi_encode()
        .into(),
    }
    .abi_encode()
    .into();
    let repayment_envelope: Bytes = ProofEnvelope {
        version: 1,
        kind: 1,
        payload: VerifiedRepayment {
            orderId: order_id.0,
            originChainId: U256::from(ORIGIN.0),
            originSettler: settler(ORIGIN),
            maker: maker(DESTINATION),
            repaymentToken: quote.origin.input_token,
            repaymentAmount: quote.amount_in,
            repaymentId: repayment_id,
        }
        .abi_encode()
        .into(),
    }
    .abi_encode()
    .into();
    let claim_origin = match route {
        CrossChainRoute::Direct => OriginSettler::settleDirectCall {
            order: order.clone(),
            mandate: mandate.clone(),
            proof: fill_id.abi_encode().into(),
            claim: claim(),
        }
        .abi_encode(),
        CrossChainRoute::Cctp => OriginSettler::settleRoutedCall {
            order: order.clone(),
            mandate: mandate.clone(),
            proof: fill_id.abi_encode().into(),
            claim: claim(),
            makerOrder: swap_order(ORIGIN),
        }
        .abi_encode(),
    };
    let mut origin_steps = vec![PreparedStep {
        command: RemoteCommand::ClaimOrigin,
        target: settler(ORIGIN),
        value: U256::ZERO,
        calldata: claim_origin.into(),
    }];
    if route == CrossChainRoute::Direct {
        origin_steps.push(PreparedStep {
            command: RemoteCommand::DispatchRepayment,
            target: outbox(ORIGIN),
            value: U256::ZERO,
            calldata: ProofOutbox::dispatchCall {
                orderId: order_id.0,
                envelope: repayment_envelope,
            }
            .abi_encode()
            .into(),
        });
    }
    let mut destination_steps = vec![
        PreparedStep {
            command: RemoteCommand::Deliver,
            target: app(DESTINATION),
            value: U256::ZERO,
            calldata: deliver.into(),
        },
        PreparedStep {
            command: RemoteCommand::DispatchFillProof,
            target: outbox(DESTINATION),
            value: U256::ZERO,
            calldata: ProofOutbox::dispatchCall {
                orderId: order_id.0,
                envelope: fill_envelope,
            }
            .abi_encode()
            .into(),
        },
    ];
    if route == CrossChainRoute::Direct {
        destination_steps.push(PreparedStep {
            command: RemoteCommand::CloseDestination,
            target: app(DESTINATION),
            value: U256::ZERO,
            calldata: DestinationApp::confirmDirectRepaymentCall {
                orderId: order_id.0,
                proof: repayment_id.abi_encode().into(),
            }
            .abi_encode()
            .into(),
        });
    }
    (
        order_id,
        ChainExecutionPlan {
            aggregate_id: quote.id,
            chain_id: ORIGIN,
            steps: origin_steps,
        },
        ChainExecutionPlan {
            aggregate_id: quote.id,
            chain_id: DESTINATION,
            steps: destination_steps,
        },
    )
}

async fn quote(proxy: &CrossChainProxy, route: CrossChainRoute) -> AggregateQuote {
    let request_id = B256::from(
        [if route == CrossChainRoute::Direct {
            0x31
        } else {
            0x32
        }; 32],
    );
    proxy
        .quote(
            &LegQuoteRequest {
                request_id,
                role: LegRole::Origin,
                local_chain: ORIGIN,
                remote_chain: DESTINATION,
                input_token: Address::repeat_byte(0xa1),
                output_token: Address::repeat_byte(0xc1),
                amount: U256::from(1_000),
                deadline_unix: NOW + 300,
                route,
            },
            &LegQuoteRequest {
                request_id,
                role: LegRole::Destination,
                local_chain: DESTINATION,
                remote_chain: ORIGIN,
                input_token: Address::repeat_byte(0xc2),
                output_token: Address::repeat_byte(0xb2),
                amount: U256::from(900),
                deadline_unix: NOW + 300,
                route,
            },
            NOW,
        )
        .await
        .expect("quote route")
}

async fn assert_checkpoint(
    runtime: &Runtime,
    route: CrossChainRoute,
    order_id: CrossChainOrderId,
    state: SagaState,
    submitted: usize,
    after_delivery: bool,
) -> CrossChainSaga {
    let saga = runtime.proxy.status(order_id).await.expect("load order");
    assert_eq!(saga.state, state);
    let recoverable = runtime
        .proxy
        .recoverable()
        .await
        .expect("recoverable sagas");
    if state == SagaState::Complete {
        assert!(recoverable.is_empty());
    } else {
        assert_eq!(recoverable, vec![saga.clone()]);
    }
    let origin = execution_stats(&runtime.origin.pool).await;
    let destination = execution_stats(&runtime.destination.pool).await;
    let count = origin.0 + destination.0;
    let distinct = origin.1 + destination.1;
    let duplicate_attempts = origin.2 + destination.2;
    assert_eq!(count, submitted as i64);
    assert_eq!(distinct, count);
    assert_eq!(duplicate_attempts, 0);
    let mut actual = execution_intents(&runtime.origin.pool).await;
    actual.extend(execution_intents(&runtime.destination.pool).await);
    actual.sort_unstable();
    let mut expected = expected_commands(route)
        .into_iter()
        .take(submitted)
        .map(|(chain, command)| step_id(order_id, chain, command).0)
        .collect::<Vec<_>>();
    expected.sort_unstable();
    assert_eq!(actual, expected);
    if after_delivery {
        for pool in [&runtime.origin.pool, &runtime.destination.pool] {
            let released: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM crosschain_preparation WHERE state = 'released'",
            )
            .fetch_one(pool)
            .await
            .expect("released preparations");
            let voided: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM ledger_reservation WHERE state = 'voided'",
            )
            .fetch_one(pool)
            .await
            .expect("voided reservations");
            assert_eq!((released, voided), (0, 0));
        }
    }
    saga
}

async fn execution_stats(pool: &SqlitePool) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT COUNT(*), COUNT(DISTINCT hex(intent)),
                COALESCE(SUM(CASE WHEN attempts != 1 THEN 1 ELSE 0 END), 0)
         FROM restart_execution",
    )
    .fetch_one(pool)
    .await
    .expect("execution submission counts")
}

async fn execution_intents(pool: &SqlitePool) -> Vec<B256> {
    let rows: Vec<Vec<u8>> = sqlx::query_scalar("SELECT intent FROM restart_execution")
        .fetch_all(pool)
        .await
        .expect("durable execution intents");
    rows.into_iter()
        .map(|row| B256::try_from(row.as_slice()).expect("32-byte execution intent"))
        .collect()
}

fn expected_commands(route: CrossChainRoute) -> Vec<(ChainId, RemoteCommand)> {
    let mut commands = vec![
        (DESTINATION, RemoteCommand::Deliver),
        (DESTINATION, RemoteCommand::DispatchFillProof),
        (ORIGIN, RemoteCommand::ClaimOrigin),
    ];
    if route == CrossChainRoute::Direct {
        commands.push((ORIGIN, RemoteCommand::DispatchRepayment));
    }
    commands.push((DESTINATION, RemoteCommand::CloseDestination));
    commands
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

fn proof_id(chain: ChainId, application: Address, order: CrossChainOrderId, kind: u8) -> B256 {
    keccak256(
        ProofIdMaterial {
            chainId: U256::from(chain.0),
            application,
            orderId: order.0,
            kind,
        }
        .abi_encode(),
    )
}

async fn advance_after_restart(
    paths: &DatabasePaths,
    route: CrossChainRoute,
    order_id: CrossChainOrderId,
    states: (SagaState, SagaState),
    behavior: (bool, bool),
    submitted: usize,
) -> CrossChainSaga {
    let (before, after) = states;
    let (confirm_on_tick, cctp_ready) = behavior;
    let runtime = boot(paths, route, confirm_on_tick, false, cctp_ready).await;
    assert_eq!(
        runtime
            .proxy
            .status(order_id)
            .await
            .expect("load order")
            .state,
        before
    );
    assert_eq!(
        runtime
            .proxy
            .recoverable()
            .await
            .expect("recoverable")
            .len(),
        1
    );
    let saga = runtime.proxy.advance(order_id).await.expect("resume order");
    assert_eq!(saga.state, after);
    let saga = assert_checkpoint(
        &runtime,
        route,
        order_id,
        after,
        submitted,
        matches!(
            after,
            SagaState::DestinationFinalized
                | SagaState::FillProofPending
                | SagaState::OriginPending
                | SagaState::OriginFinalized
                | SagaState::RepaymentPending
                | SagaState::Complete
        ),
    )
    .await;
    runtime.shutdown().await;
    saga
}

async fn recovery_scenario(route: CrossChainRoute) {
    let directory = tempfile::tempdir().expect("restart database directory");
    let paths = DatabasePaths::new(directory.path());
    let runtime = boot(&paths, route, false, true, false).await;
    let quote = quote(&runtime.proxy, route).await;
    let (order_id, origin_plan, destination_plan) = plans(route, &quote);
    assert!(runtime
        .proxy
        .start(
            order_id,
            quote.clone(),
            &origin_plan,
            &destination_plan,
            NOW,
        )
        .await
        .is_err());
    let partial =
        assert_checkpoint(&runtime, route, order_id, SagaState::Preparing, 0, false).await;
    assert!(partial.origin_prepare.is_some());
    assert!(partial.destination_prepare.is_none());
    runtime.shutdown().await;

    let runtime = boot(&paths, route, false, false, false).await;
    let prepared = runtime
        .proxy
        .advance(order_id)
        .await
        .expect("finish prepare");
    assert_eq!(prepared.state, SagaState::Prepared);
    assert_checkpoint(&runtime, route, order_id, SagaState::Prepared, 0, false).await;
    runtime.shutdown().await;

    advance_after_restart(
        &paths,
        route,
        order_id,
        (SagaState::Prepared, SagaState::DestinationPending),
        (false, false),
        1,
    )
    .await;
    advance_after_restart(
        &paths,
        route,
        order_id,
        (
            SagaState::DestinationPending,
            SagaState::DestinationFinalized,
        ),
        (true, false),
        1,
    )
    .await;
    advance_after_restart(
        &paths,
        route,
        order_id,
        (SagaState::DestinationFinalized, SagaState::FillProofPending),
        (false, false),
        2,
    )
    .await;
    advance_after_restart(
        &paths,
        route,
        order_id,
        (SagaState::FillProofPending, SagaState::OriginPending),
        (true, false),
        2,
    )
    .await;
    advance_after_restart(
        &paths,
        route,
        order_id,
        (SagaState::OriginPending, SagaState::OriginPending),
        (false, false),
        3,
    )
    .await;
    advance_after_restart(
        &paths,
        route,
        order_id,
        (SagaState::OriginPending, SagaState::OriginFinalized),
        (true, false),
        3,
    )
    .await;

    match route {
        CrossChainRoute::Direct => {
            advance_after_restart(
                &paths,
                route,
                order_id,
                (SagaState::OriginFinalized, SagaState::OriginFinalized),
                (false, false),
                4,
            )
            .await;
            advance_after_restart(
                &paths,
                route,
                order_id,
                (SagaState::OriginFinalized, SagaState::RepaymentPending),
                (true, false),
                4,
            )
            .await;
        }
        CrossChainRoute::Cctp => {
            let runtime = boot(&paths, route, true, false, false).await;
            assert_checkpoint(
                &runtime,
                route,
                order_id,
                SagaState::OriginFinalized,
                3,
                true,
            )
            .await;
            let pending = runtime
                .proxy
                .advance(order_id)
                .await
                .expect("poll attestation");
            assert_eq!(pending.state, SagaState::OriginFinalized);
            assert_checkpoint(
                &runtime,
                route,
                order_id,
                SagaState::OriginFinalized,
                3,
                true,
            )
            .await;
            runtime.shutdown().await;

            let runtime = boot(&paths, route, true, false, true).await;
            let repayment = runtime
                .proxy
                .advance(order_id)
                .await
                .expect("stage CCTP close");
            assert_eq!(repayment.state, SagaState::RepaymentPending);
            assert_eq!(
                repayment
                    .repayment
                    .as_ref()
                    .and_then(|item| item.message_id),
                Some(B256::repeat_byte(0xcc))
            );
            assert_checkpoint(
                &runtime,
                route,
                order_id,
                SagaState::RepaymentPending,
                3,
                true,
            )
            .await;
            let polls: i64 =
                sqlx::query_scalar("SELECT attempts FROM restart_cctp_poll WHERE order_id = ?")
                    .bind(order_id.0.to_vec())
                    .fetch_one(&runtime.saga_pool)
                    .await
                    .expect("CCTP polling attempts");
            assert_eq!(polls, 2);
            runtime.shutdown().await;
        }
    }

    advance_after_restart(
        &paths,
        route,
        order_id,
        (SagaState::RepaymentPending, SagaState::RepaymentPending),
        (false, true),
        if route == CrossChainRoute::Direct {
            5
        } else {
            4
        },
    )
    .await;
    let submitted = if route == CrossChainRoute::Direct {
        5
    } else {
        4
    };
    let complete = advance_after_restart(
        &paths,
        route,
        order_id,
        (SagaState::RepaymentPending, SagaState::Complete),
        (true, true),
        submitted,
    )
    .await;
    assert_eq!(complete.state, SagaState::Complete);

    let runtime = boot(&paths, route, true, false, true).await;
    assert_checkpoint(
        &runtime,
        route,
        order_id,
        SagaState::Complete,
        submitted,
        true,
    )
    .await;
    assert_eq!(
        runtime
            .proxy
            .advance(order_id)
            .await
            .expect("replay complete"),
        complete
    );
    assert_checkpoint(
        &runtime,
        route,
        order_id,
        SagaState::Complete,
        submitted,
        true,
    )
    .await;
    runtime.shutdown().await;
}

#[tokio::test]
async fn direct_route_recovers_from_every_durable_checkpoint() {
    recovery_scenario(CrossChainRoute::Direct).await;
}

#[tokio::test]
async fn cctp_route_recovers_before_and_after_attestation() {
    recovery_scenario(CrossChainRoute::Cctp).await;
}
