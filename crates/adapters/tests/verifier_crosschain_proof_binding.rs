use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use alloy::primitives::{keccak256, Address, Bytes, Uint, B256, U256};
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolCall, SolStruct, SolValue};
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
    AggregateQuote, ChainExecutionPlan, CrossChainRoute, LegQuote, LegQuoteRequest, LegRole,
    PreparedStep, RemoteCommand, StepValidationContext,
};
use solvent_core::primitives::execution::{
    ExecHandle, ExecStatus, FillTx, SimVerdict, TrackedFill,
};
use solvent_core::primitives::ledger::{AccountKey, ReservationSource};
use solvent_core::primitives::{
    AggregateQuoteId, ChainId, CrossChainOrderId, IntentId, MakerId, ReservationId, StrategyHash,
};
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

type U48 = Uint<48, 1>;

const NOW: u64 = 1_800_000_000;
const ORIGIN_CHAIN: ChainId = ChainId(1);
const DESTINATION_CHAIN: ChainId = ChainId(42_161);
const ORIGIN_TOKEN: Address = Address::repeat_byte(0x11);
const DESTINATION_TOKEN: Address = Address::repeat_byte(0x12);
const DESTINATION_APP: Address = Address::repeat_byte(0x21);
const ORIGIN_SETTLER: Address = Address::repeat_byte(0x22);
const DESTINATION_OUTBOX: Address = Address::repeat_byte(0x23);
const ORIGIN_OUTBOX: Address = Address::repeat_byte(0x24);
const OPERATOR: Address = Address::repeat_byte(0x25);
const MAKER: Address = Address::repeat_byte(0x31);
const DESTINATION_STRATEGY: B256 = B256::repeat_byte(0x32);
const ORIGIN_STRATEGY: B256 = B256::repeat_byte(0x33);

sol! {
    struct SolventCrossChainOrder {
        address user;
        uint256 nonce;
        uint256 originChainId;
        address originSettler;
        address compact;
        uint256 compactId;
        uint256 compactExpires;
        address inputToken;
        uint256 inputAmount;
        uint256 destinationChainId;
        address outputToken;
        uint256 minimumOutputAmount;
        address recipient;
        address destinationSettler;
        address fillProofVerifier;
        address exclusiveFiller;
        uint48 exclusivityEnds;
        uint48 fillDeadline;
        uint8 routeKind;
    }

    struct SolventMandate {
        bytes32 orderId;
        uint256 destinationChainId;
        address destinationSettler;
        address fillProofVerifier;
        address outputToken;
        uint256 minimumOutputAmount;
        address recipient;
        uint48 fillDeadline;
        address exclusiveFiller;
        uint8 routeKind;
    }

    struct DirectMakerQuote {
        bytes32 orderId;
        address maker;
        bytes32 destinationStrategyHash;
        bytes32 originStrategyHash;
        uint256 outputAmount;
        uint256 repaymentAmount;
        uint256 nonce;
        uint48 expires;
    }

    struct Component {
        uint256 claimant;
        uint256 amount;
    }

    struct Claim {
        bytes allocatorData;
        bytes sponsorSignature;
        address sponsor;
        uint256 nonce;
        uint256 expires;
        bytes32 witness;
        string witnessTypestring;
        uint256 id;
        uint256 allocatedAmount;
        Component[] claimants;
    }

    struct ProofEnvelope {
        uint8 version;
        uint8 kind;
        bytes payload;
    }

    struct ProofIdMaterial {
        uint256 chainId;
        address application;
        bytes32 orderId;
        uint8 kind;
    }

    struct VerifiedFill {
        bytes32 orderId;
        uint8 routeKind;
        uint256 destinationChainId;
        address destinationApp;
        address recipient;
        address outputToken;
        uint256 outputAmount;
        address destinationMaker;
        bytes32 destinationStrategyHash;
        bytes32 originStrategyHash;
        address repaymentToken;
        uint256 repaymentAmount;
        uint256 maxCctpFee;
        bytes32 makerQuoteHash;
        bytes32 fillId;
    }

    struct VerifiedRepayment {
        bytes32 orderId;
        uint256 originChainId;
        address originSettler;
        address maker;
        address repaymentToken;
        uint256 repaymentAmount;
        bytes32 repaymentId;
    }

    interface OriginSettler {
        function settleDirect(
            SolventCrossChainOrder order,
            SolventMandate mandate,
            bytes fillProof,
            Claim compactClaim
        ) external;
    }

    interface ProofOutbox {
        function dispatch(bytes32 orderId, bytes envelope) external payable returns (bytes32);
    }

    interface DestinationApp {
        function fillDirect(
            SolventCrossChainOrder order,
            SolventMandate mandate,
            DirectMakerQuote quote,
            bytes makerSignature
        ) external;
        function confirmDirectRepayment(bytes32 orderId, bytes repaymentProof) external;
        function completeRepayment(bytes32 orderId, bytes message, bytes attestation) external;
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

struct DirectQuoter;

#[async_trait]
impl LegQuoter for DirectQuoter {
    async fn quote(&self, request: &LegQuoteRequest) -> Result<LegQuote, LegQuoterError> {
        let amount_out = match request.role {
            LegRole::Origin => U256::from(1_000),
            LegRole::Destination => U256::from(850),
        };
        let sources = match request.role {
            LegRole::Origin => Vec::new(),
            LegRole::Destination => vec![ReservationSource {
                maker: MakerId(MAKER),
                strategy_hash: StrategyHash(DESTINATION_STRATEGY),
                token: DESTINATION_TOKEN,
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
            block_number: 100,
            expires_at_unix: NOW + 60,
            sources,
        };
        quote.quote_id = leg_quote_id(&quote);
        Ok(quote)
    }
}

#[derive(Default)]
struct ActionProbe {
    simulations: AtomicUsize,
    submissions: AtomicUsize,
}

#[async_trait]
impl SimGate for ActionProbe {
    async fn simulate(&self, _: &FillTx) -> Result<SimVerdict, SimError> {
        self.simulations.fetch_add(1, Ordering::SeqCst);
        Ok(SimVerdict::Ok)
    }
}

#[async_trait]
impl Execution for ActionProbe {
    async fn submit(&self, _: &FillTx, _: ReservationId) -> Result<ExecHandle, ExecutionError> {
        self.submissions.fetch_add(1, Ordering::SeqCst);
        Ok(ExecHandle(B256::ZERO))
    }

    async fn status(&self, _: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError> {
        Ok(None)
    }

    async fn forget(&self, _: IntentId) -> Result<(), ExecutionError> {
        Ok(())
    }

    async fn tracked(&self) -> Result<Vec<TrackedFill>, ExecutionError> {
        Ok(Vec::new())
    }

    async fn tick(&self) -> Result<(), ExecutionError> {
        Ok(())
    }
}

struct StageOutcome {
    status: StatusCode,
    persisted: bool,
    bound: bool,
    simulations: usize,
    submissions: usize,
}

fn quote_request(role: LegRole, request_id: B256) -> LegQuoteRequest {
    match role {
        LegRole::Origin => LegQuoteRequest {
            request_id,
            role,
            local_chain: ORIGIN_CHAIN,
            remote_chain: DESTINATION_CHAIN,
            input_token: ORIGIN_TOKEN,
            output_token: ORIGIN_TOKEN,
            amount: U256::from(1_000),
            deadline_unix: NOW + 60,
            route: CrossChainRoute::Direct,
        },
        LegRole::Destination => LegQuoteRequest {
            request_id,
            role,
            local_chain: DESTINATION_CHAIN,
            remote_chain: ORIGIN_CHAIN,
            input_token: ORIGIN_TOKEN,
            output_token: DESTINATION_TOKEN,
            amount: U256::from(1_000),
            deadline_unix: NOW + 60,
            route: CrossChainRoute::Direct,
        },
    }
}

fn remote_quote(role: LegRole, request_id: B256) -> LegQuote {
    let request = quote_request(role, request_id);
    let amount_out = match role {
        LegRole::Origin => U256::from(1_000),
        LegRole::Destination => U256::from(850),
    };
    let sources = match role {
        LegRole::Origin => Vec::new(),
        LegRole::Destination => vec![ReservationSource {
            maker: MakerId(MAKER),
            strategy_hash: StrategyHash(DESTINATION_STRATEGY),
            token: DESTINATION_TOKEN,
            amount: amount_out,
        }],
    };
    let mut quote = LegQuote {
        quote_id: B256::ZERO,
        request_id,
        role,
        local_chain: request.local_chain,
        remote_chain: request.remote_chain,
        input_token: request.input_token,
        output_token: request.output_token,
        amount_in: request.amount,
        amount_out,
        route: request.route,
        price_impact_bps: None,
        block_number: 99,
        expires_at_unix: NOW + 60,
        sources,
    };
    quote.quote_id = leg_quote_id(&quote);
    quote
}

fn order(quote: &AggregateQuote) -> SolventCrossChainOrder {
    SolventCrossChainOrder {
        user: Address::repeat_byte(0x41),
        nonce: U256::from(1),
        originChainId: U256::from(quote.origin.local_chain.0),
        originSettler: ORIGIN_SETTLER,
        compact: Address::repeat_byte(0x42),
        compactId: U256::from(2),
        compactExpires: U256::from(NOW + 60),
        inputToken: quote.origin.input_token,
        inputAmount: quote.amount_in,
        destinationChainId: U256::from(quote.destination.local_chain.0),
        outputToken: quote.destination.output_token,
        minimumOutputAmount: quote.amount_out,
        recipient: Address::repeat_byte(0x43),
        destinationSettler: DESTINATION_APP,
        fillProofVerifier: Address::repeat_byte(0x44),
        exclusiveFiller: OPERATOR,
        exclusivityEnds: U48::from(NOW + 30),
        fillDeadline: U48::from(NOW + 50),
        routeKind: 1,
    }
}

fn mandate(order: &SolventCrossChainOrder, order_id: CrossChainOrderId) -> SolventMandate {
    SolventMandate {
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
    }
}

fn fill_id(order_id: CrossChainOrderId) -> B256 {
    keccak256(
        ProofIdMaterial {
            chainId: U256::from(DESTINATION_CHAIN.0),
            application: DESTINATION_APP,
            orderId: order_id.0,
            kind: 0,
        }
        .abi_encode(),
    )
}

fn repayment_id(order_id: CrossChainOrderId) -> B256 {
    keccak256(
        ProofIdMaterial {
            chainId: U256::from(ORIGIN_CHAIN.0),
            application: ORIGIN_SETTLER,
            orderId: order_id.0,
            kind: 1,
        }
        .abi_encode(),
    )
}

fn maker_quote(order_id: CrossChainOrderId) -> DirectMakerQuote {
    DirectMakerQuote {
        orderId: order_id.0,
        maker: MAKER,
        destinationStrategyHash: DESTINATION_STRATEGY,
        originStrategyHash: ORIGIN_STRATEGY,
        outputAmount: U256::from(850),
        repaymentAmount: U256::from(1_000),
        nonce: U256::from(3),
        expires: U48::from(NOW + 50),
    }
}

fn maker_quote_hash(order_id: CrossChainOrderId) -> B256 {
    let domain = eip712_domain! {
        name: "Solvent Cross-Chain Aqua App",
        version: "1",
        chain_id: DESTINATION_CHAIN.0,
        verifying_contract: DESTINATION_APP,
    };
    maker_quote(order_id).eip712_signing_hash(&domain)
}

fn bind_cctp_terms(mut quote: AggregateQuote) -> AggregateQuote {
    quote.cctp_finality_threshold = Some(2_000);
    let mut bytes = Vec::with_capacity(100);
    bytes.extend_from_slice(quote.origin.quote_id.as_slice());
    bytes.extend_from_slice(quote.destination.quote_id.as_slice());
    bytes.extend_from_slice(&quote.bridge_fee.to_be_bytes::<32>());
    bytes.extend_from_slice(&2_000_u32.to_be_bytes());
    quote.id = AggregateQuoteId(keccak256(bytes));
    quote
}

fn valid_fill(
    quote: &AggregateQuote,
    order: &SolventCrossChainOrder,
    order_id: CrossChainOrderId,
) -> VerifiedFill {
    VerifiedFill {
        orderId: order_id.0,
        routeKind: 1,
        destinationChainId: U256::from(DESTINATION_CHAIN.0),
        destinationApp: DESTINATION_APP,
        recipient: order.recipient,
        outputToken: quote.destination.output_token,
        outputAmount: quote.amount_out,
        destinationMaker: MAKER,
        destinationStrategyHash: DESTINATION_STRATEGY,
        originStrategyHash: ORIGIN_STRATEGY,
        repaymentToken: quote.origin.input_token,
        repaymentAmount: quote.amount_in,
        maxCctpFee: U256::ZERO,
        makerQuoteHash: maker_quote_hash(order_id),
        fillId: fill_id(order_id),
    }
}

fn valid_repayment(quote: &AggregateQuote, order_id: CrossChainOrderId) -> VerifiedRepayment {
    VerifiedRepayment {
        orderId: order_id.0,
        originChainId: U256::from(ORIGIN_CHAIN.0),
        originSettler: ORIGIN_SETTLER,
        maker: MAKER,
        repaymentToken: quote.origin.input_token,
        repaymentAmount: quote.amount_in,
        repaymentId: repayment_id(order_id),
    }
}

fn envelope(kind: u8, payload: Vec<u8>) -> Bytes {
    ProofEnvelope {
        version: 1,
        kind,
        payload: payload.into(),
    }
    .abi_encode()
    .into()
}

fn target(command: RemoteCommand) -> (ChainId, LegRole, Address) {
    match command {
        RemoteCommand::ClaimOrigin => (ORIGIN_CHAIN, LegRole::Origin, ORIGIN_SETTLER),
        RemoteCommand::DispatchRepayment => (ORIGIN_CHAIN, LegRole::Origin, ORIGIN_OUTBOX),
        RemoteCommand::DispatchFillProof => {
            (DESTINATION_CHAIN, LegRole::Destination, DESTINATION_OUTBOX)
        }
        RemoteCommand::CloseDestination => {
            (DESTINATION_CHAIN, LegRole::Destination, DESTINATION_APP)
        }
        RemoteCommand::Deliver => unreachable!("delivery is outside this verifier"),
    }
}

async fn stage_case(
    command: RemoteCommand,
    make_step: impl FnOnce(&AggregateQuote, &SolventCrossChainOrder, CrossChainOrderId) -> PreparedStep,
) -> StageOutcome {
    stage_case_with_shape(command, true, make_step).await
}

async fn stage_case_with_shape(
    command: RemoteCommand,
    complete_plan: bool,
    make_step: impl FnOnce(&AggregateQuote, &SolventCrossChainOrder, CrossChainOrderId) -> PreparedStep,
) -> StageOutcome {
    let (chain_id, role, _) = target(command);
    let request_id = B256::repeat_byte(command as u8 + 0x51);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open verifier SQLite");
    let preparations = Arc::new(SqlitePreparationStore::new(pool.clone()));
    preparations.migrate().await.expect("apply schema");
    let clock: Arc<dyn Clock> = Arc::new(FixedClock);
    let local = Arc::new(LocalCrossChainService::new(
        chain_id,
        Arc::new(DirectQuoter),
        Arc::new(SqliteLegQuoteStore::new(pool.clone())),
        preparations,
        Arc::new(LedgerService::new(
            Arc::new(SqliteLedgerStore::new(pool.clone())),
            Arc::new(UnlimitedBudget),
            Arc::clone(&clock),
        )),
        clock,
    ));
    let local_quote = local
        .quote(&quote_request(role, request_id))
        .await
        .expect("issue local quote");
    let quote = match role {
        LegRole::Origin => aggregate_quote(
            local_quote,
            remote_quote(LegRole::Destination, request_id),
            NOW,
        ),
        LegRole::Destination => {
            aggregate_quote(remote_quote(LegRole::Origin, request_id), local_quote, NOW)
        }
    }
    .expect("aggregate direct quote");
    let settlement_order = order(&quote);
    let order_id = CrossChainOrderId(settlement_order.eip712_hash_struct());
    let step = make_step(&quote, &settlement_order, order_id);
    let steps = if complete_plan {
        match role {
            LegRole::Origin => vec![
                if command == RemoteCommand::ClaimOrigin {
                    step.clone()
                } else {
                    valid_claim_step(&quote, &settlement_order, order_id)
                },
                if command == RemoteCommand::DispatchRepayment {
                    step
                } else {
                    valid_repayment_dispatch_step(&quote, order_id)
                },
            ],
            LegRole::Destination => vec![
                valid_delivery_step(&settlement_order, order_id),
                if command == RemoteCommand::DispatchFillProof {
                    step.clone()
                } else {
                    valid_fill_dispatch_step(&quote, &settlement_order, order_id)
                },
                if command == RemoteCommand::CloseDestination {
                    step
                } else {
                    close_step(order_id, repayment_id(order_id).abi_encode().into())
                },
            ],
        }
    } else {
        vec![step]
    };
    let plan = ChainExecutionPlan {
        aggregate_id: quote.id,
        chain_id,
        steps,
    };
    let context = StepValidationContext {
        order_id,
        quote,
        role,
    };
    let store = Arc::new(SqliteStepStore::new(pool));
    let probe = Arc::new(ActionProbe::default());
    let targets = match role {
        LegRole::Origin => BTreeMap::from([
            (RemoteCommand::ClaimOrigin, ORIGIN_SETTLER),
            (RemoteCommand::DispatchRepayment, ORIGIN_OUTBOX),
        ]),
        LegRole::Destination => BTreeMap::from([
            (RemoteCommand::Deliver, DESTINATION_APP),
            (RemoteCommand::DispatchFillProof, DESTINATION_OUTBOX),
            (RemoteCommand::CloseDestination, DESTINATION_APP),
        ]),
    };
    let steps = Arc::new(
        LocalStepService::new(
            chain_id,
            OPERATOR,
            targets,
            store.clone(),
            probe.clone(),
            probe.clone(),
        )
        .with_validator(Arc::new(
            AlloyStepValidator::new(OPERATOR, Address::repeat_byte(0x26))
                .with_applications(ORIGIN_SETTLER, DESTINATION_APP),
        )),
    );
    let router = crosschain_internal_router(
        local,
        steps,
        HeaderValue::from_static("Bearer verifier-proof"),
    );
    let body = serde_json::to_vec(&serde_json::json!({
        "context": context,
        "plan": plan,
    }))
    .expect("serialize stage request");
    let response = router
        .oneshot(
            Request::post("/internal/v1/cross-chain/stage")
                .header("authorization", "Bearer verifier-proof")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("build stage request"),
        )
        .await
        .expect("serve stage request");
    let persisted = store
        .load(order_id, command)
        .await
        .expect("inspect staged step")
        .is_some();
    let bound = store
        .aggregate_id(order_id, command)
        .await
        .expect("inspect aggregate binding")
        .is_some();
    StageOutcome {
        status: response.status(),
        persisted,
        bound,
        simulations: probe.simulations.load(Ordering::SeqCst),
        submissions: probe.submissions.load(Ordering::SeqCst),
    }
}

fn valid_delivery_step(
    order: &SolventCrossChainOrder,
    order_id: CrossChainOrderId,
) -> PreparedStep {
    PreparedStep {
        command: RemoteCommand::Deliver,
        target: DESTINATION_APP,
        value: U256::ZERO,
        calldata: DestinationApp::fillDirectCall {
            order: order.clone(),
            mandate: mandate(order, order_id),
            quote: maker_quote(order_id),
            makerSignature: Bytes::from_static(b"signed-maker-quote"),
        }
        .abi_encode()
        .into(),
    }
}

fn valid_claim_step(
    quote: &AggregateQuote,
    order: &SolventCrossChainOrder,
    order_id: CrossChainOrderId,
) -> PreparedStep {
    claim_step(
        quote,
        order,
        order_id,
        fill_id(order_id).abi_encode().into(),
    )
}

fn valid_fill_dispatch_step(
    quote: &AggregateQuote,
    order: &SolventCrossChainOrder,
    order_id: CrossChainOrderId,
) -> PreparedStep {
    dispatch_step(
        RemoteCommand::DispatchFillProof,
        order_id,
        envelope(0, valid_fill(quote, order, order_id).abi_encode()),
    )
}

fn valid_repayment_dispatch_step(
    quote: &AggregateQuote,
    order_id: CrossChainOrderId,
) -> PreparedStep {
    dispatch_step(
        RemoteCommand::DispatchRepayment,
        order_id,
        envelope(1, valid_repayment(quote, order_id).abi_encode()),
    )
}

fn claim_step(
    quote: &AggregateQuote,
    order: &SolventCrossChainOrder,
    order_id: CrossChainOrderId,
    proof: Bytes,
) -> PreparedStep {
    PreparedStep {
        command: RemoteCommand::ClaimOrigin,
        target: ORIGIN_SETTLER,
        value: U256::ZERO,
        calldata: OriginSettler::settleDirectCall {
            order: order.clone(),
            mandate: mandate(order, order_id),
            fillProof: proof,
            compactClaim: Claim {
                allocatorData: Bytes::new(),
                sponsorSignature: Bytes::new(),
                sponsor: order.user,
                nonce: U256::from(1),
                expires: U256::from(NOW + 50),
                witness: B256::ZERO,
                witnessTypestring: String::new(),
                id: U256::from(1),
                allocatedAmount: quote.amount_in,
                claimants: Vec::new(),
            },
        }
        .abi_encode()
        .into(),
    }
}

fn dispatch_step(
    command: RemoteCommand,
    order_id: CrossChainOrderId,
    envelope: Bytes,
) -> PreparedStep {
    let (_, _, target) = target(command);
    PreparedStep {
        command,
        target,
        value: U256::ZERO,
        calldata: ProofOutbox::dispatchCall {
            orderId: order_id.0,
            envelope,
        }
        .abi_encode()
        .into(),
    }
}

fn close_step(order_id: CrossChainOrderId, proof: Bytes) -> PreparedStep {
    PreparedStep {
        command: RemoteCommand::CloseDestination,
        target: DESTINATION_APP,
        value: U256::ZERO,
        calldata: DestinationApp::confirmDirectRepaymentCall {
            orderId: order_id.0,
            repaymentProof: proof,
        }
        .abi_encode()
        .into(),
    }
}

async fn cctp_completion_case(executed: bool, dedicated_endpoint: bool) -> StageOutcome {
    let request_id = B256::repeat_byte(0xb1);
    let mut destination_request = quote_request(LegRole::Destination, request_id);
    destination_request.route = CrossChainRoute::Cctp;
    let mut origin = remote_quote(LegRole::Origin, request_id);
    origin.route = CrossChainRoute::Cctp;
    origin.quote_id = leg_quote_id(&origin);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open CCTP verifier SQLite");
    let preparations = Arc::new(SqlitePreparationStore::new(pool.clone()));
    preparations.migrate().await.expect("apply schema");
    let clock: Arc<dyn Clock> = Arc::new(FixedClock);
    let local = Arc::new(LocalCrossChainService::new(
        DESTINATION_CHAIN,
        Arc::new(DirectQuoter),
        Arc::new(SqliteLegQuoteStore::new(pool.clone())),
        preparations,
        Arc::new(LedgerService::new(
            Arc::new(SqliteLedgerStore::new(pool.clone())),
            Arc::new(UnlimitedBudget),
            Arc::clone(&clock),
        )),
        clock,
    ));
    let destination = local
        .quote(&destination_request)
        .await
        .expect("issue destination CCTP quote");
    let quote =
        bind_cctp_terms(aggregate_quote(origin, destination, NOW).expect("aggregate CCTP quote"));
    let preparation = local
        .prepare(quote.id, &quote.destination)
        .await
        .expect("prepare destination capital");
    let preparation = if executed {
        local
            .commit(preparation.token)
            .await
            .expect("commit destination capital");
        local
            .mark_executed(preparation.token)
            .await
            .expect("mark delivered capital executed")
    } else {
        preparation
    };
    let order_id = CrossChainOrderId(B256::repeat_byte(0xb2));
    let command = RemoteCommand::CloseDestination;
    let plan = ChainExecutionPlan {
        aggregate_id: quote.id,
        chain_id: DESTINATION_CHAIN,
        steps: vec![PreparedStep {
            command,
            target: DESTINATION_APP,
            value: U256::ZERO,
            calldata: DestinationApp::completeRepaymentCall {
                orderId: order_id.0,
                message: Bytes::from_static(b"cctp-message"),
                attestation: Bytes::from_static(b"cctp-attestation"),
            }
            .abi_encode()
            .into(),
        }],
    };
    let context = StepValidationContext {
        order_id,
        quote,
        role: LegRole::Destination,
    };
    let store = Arc::new(SqliteStepStore::new(pool));
    let probe = Arc::new(ActionProbe::default());
    let steps = Arc::new(
        LocalStepService::new(
            DESTINATION_CHAIN,
            OPERATOR,
            BTreeMap::from([(command, DESTINATION_APP)]),
            store.clone(),
            probe.clone(),
            probe.clone(),
        )
        .with_validator(Arc::new(
            AlloyStepValidator::new(OPERATOR, Address::repeat_byte(0x26))
                .with_applications(ORIGIN_SETTLER, DESTINATION_APP),
        )),
    );
    let router = crosschain_internal_router(
        local,
        steps,
        HeaderValue::from_static("Bearer verifier-proof"),
    );
    let body = serde_json::to_vec(&serde_json::json!({
        "context": context,
        "plan": plan,
        "preparation": preparation.token,
    }))
    .expect("serialize completion request");
    let response = router
        .oneshot(
            Request::post(if dedicated_endpoint {
                "/internal/v1/cross-chain/stage-cctp-completion"
            } else {
                "/internal/v1/cross-chain/stage"
            })
            .header("authorization", "Bearer verifier-proof")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .expect("build completion request"),
        )
        .await
        .expect("serve completion request");
    let persisted = store
        .load(order_id, command)
        .await
        .expect("inspect completion step")
        .is_some();
    let bound = store
        .aggregate_id(order_id, command)
        .await
        .expect("inspect completion binding")
        .is_some();
    StageOutcome {
        status: response.status(),
        persisted,
        bound,
        simulations: probe.simulations.load(Ordering::SeqCst),
        submissions: probe.submissions.load(Ordering::SeqCst),
    }
}

fn assert_valid(outcome: StageOutcome) {
    assert_eq!(
        (
            outcome.status,
            outcome.persisted,
            outcome.bound,
            outcome.simulations,
            outcome.submissions,
        ),
        (StatusCode::OK, true, true, 0, 0),
    );
}

async fn assert_rejected(name: &str, outcome: StageOutcome, accepted: &mut Vec<String>) {
    assert_eq!(
        (outcome.simulations, outcome.submissions),
        (0, 0),
        "staging {name} must not touch chain adapters",
    );
    if outcome.status == StatusCode::OK && outcome.persisted && outcome.bound {
        accepted.push(name.to_string());
    } else {
        assert_eq!(
            (outcome.status, outcome.persisted, outcome.bound),
            (StatusCode::BAD_REQUEST, false, false),
            "{name} must either be atomically rejected or expose the current acceptance gap",
        );
    }
}

#[tokio::test]
async fn valid_proof_steps_stage_without_executing_chain_actions() {
    assert_valid(
        stage_case(RemoteCommand::ClaimOrigin, |quote, order, order_id| {
            claim_step(
                quote,
                order,
                order_id,
                fill_id(order_id).abi_encode().into(),
            )
        })
        .await,
    );
    assert_valid(
        stage_case(
            RemoteCommand::DispatchFillProof,
            |quote, order, order_id| {
                dispatch_step(
                    RemoteCommand::DispatchFillProof,
                    order_id,
                    envelope(0, valid_fill(quote, order, order_id).abi_encode()),
                )
            },
        )
        .await,
    );
    assert_valid(
        stage_case(RemoteCommand::DispatchRepayment, |quote, _, order_id| {
            dispatch_step(
                RemoteCommand::DispatchRepayment,
                order_id,
                envelope(1, valid_repayment(quote, order_id).abi_encode()),
            )
        })
        .await,
    );
    assert_valid(
        stage_case(RemoteCommand::CloseDestination, |_, _, order_id| {
            close_step(order_id, repayment_id(order_id).abi_encode().into())
        })
        .await,
    );
}

#[tokio::test]
async fn standalone_fill_dispatch_is_rejected_as_an_incomplete_initial_plan() {
    let outcome = stage_case_with_shape(
        RemoteCommand::DispatchFillProof,
        false,
        valid_fill_dispatch_step,
    )
    .await;
    assert_eq!(
        (
            outcome.status,
            outcome.persisted,
            outcome.bound,
            outcome.simulations,
            outcome.submissions,
        ),
        (StatusCode::BAD_REQUEST, false, false, 0, 0),
    );
}

#[tokio::test]
async fn cctp_completion_requires_an_executed_destination_preparation() {
    let regular_stage = cctp_completion_case(true, false).await;
    assert_eq!(
        (
            regular_stage.status,
            regular_stage.persisted,
            regular_stage.bound,
            regular_stage.simulations,
            regular_stage.submissions,
        ),
        (StatusCode::BAD_REQUEST, false, false, 0, 0),
        "regular initial staging must reject a singleton CCTP close",
    );

    let early = cctp_completion_case(false, true).await;
    assert_eq!(
        (
            early.status,
            early.persisted,
            early.bound,
            early.simulations,
            early.submissions,
        ),
        (StatusCode::BAD_REQUEST, false, false, 0, 0),
        "a prepared but unexecuted destination cannot authorize CCTP completion",
    );

    assert_valid(cctp_completion_case(true, true).await);
}

#[tokio::test]
async fn malformed_or_substituted_immutable_proofs_are_rejected_before_persistence() {
    let mut accepted = Vec::new();

    for (name, proof) in [
        ("claim/wrong-fill-id", B256::repeat_byte(0x81).abi_encode()),
        ("claim/non-32-byte-proof", vec![0x82; 31]),
    ] {
        let outcome = stage_case(RemoteCommand::ClaimOrigin, |quote, order, order_id| {
            claim_step(quote, order, order_id, proof.into())
        })
        .await;
        assert_rejected(name, outcome, &mut accepted).await;
    }

    let fill_mutations = [
        "version",
        "kind",
        "order",
        "chain",
        "app",
        "amount",
        "maker",
        "strategy",
        "repayment-token",
        "repayment-amount",
        "fill-id",
    ];
    for mutation in fill_mutations {
        let outcome = stage_case(
            RemoteCommand::DispatchFillProof,
            |quote, order, order_id| {
                let mut fill = valid_fill(quote, order, order_id);
                let mut version = 1;
                let mut kind = 0;
                match mutation {
                    "version" => version = 2,
                    "kind" => kind = 1,
                    "order" => fill.orderId = B256::repeat_byte(0x83),
                    "chain" => fill.destinationChainId = U256::from(10),
                    "app" => fill.destinationApp = Address::repeat_byte(0x84),
                    "amount" => fill.outputAmount -= U256::from(1),
                    "maker" => fill.destinationMaker = Address::repeat_byte(0x85),
                    "strategy" => fill.destinationStrategyHash = B256::repeat_byte(0x86),
                    "repayment-token" => fill.repaymentToken = Address::repeat_byte(0x87),
                    "repayment-amount" => fill.repaymentAmount -= U256::from(1),
                    "fill-id" => fill.fillId = B256::repeat_byte(0x88),
                    _ => unreachable!("complete fill mutation table"),
                }
                dispatch_step(
                    RemoteCommand::DispatchFillProof,
                    order_id,
                    ProofEnvelope {
                        version,
                        kind,
                        payload: fill.abi_encode().into(),
                    }
                    .abi_encode()
                    .into(),
                )
            },
        )
        .await;
        assert_rejected(&format!("dispatch-fill/{mutation}"), outcome, &mut accepted).await;
    }

    let repayment_mutations = [
        "version",
        "kind",
        "order",
        "chain",
        "app",
        "maker",
        "repayment-token",
        "repayment-amount",
        "repayment-id",
    ];
    for mutation in repayment_mutations {
        let outcome = stage_case(RemoteCommand::DispatchRepayment, |quote, _, order_id| {
            let mut repayment = valid_repayment(quote, order_id);
            let mut version = 1;
            let mut kind = 1;
            match mutation {
                "version" => version = 2,
                "kind" => kind = 0,
                "order" => repayment.orderId = B256::repeat_byte(0x91),
                "chain" => repayment.originChainId = U256::from(10),
                "app" => repayment.originSettler = Address::repeat_byte(0x92),
                "maker" => repayment.maker = Address::repeat_byte(0x93),
                "repayment-token" => repayment.repaymentToken = Address::repeat_byte(0x94),
                "repayment-amount" => repayment.repaymentAmount -= U256::from(1),
                "repayment-id" => repayment.repaymentId = B256::repeat_byte(0x95),
                _ => unreachable!("complete repayment mutation table"),
            }
            dispatch_step(
                RemoteCommand::DispatchRepayment,
                order_id,
                ProofEnvelope {
                    version,
                    kind,
                    payload: repayment.abi_encode().into(),
                }
                .abi_encode()
                .into(),
            )
        })
        .await;
        assert_rejected(
            &format!("dispatch-repayment/{mutation}"),
            outcome,
            &mut accepted,
        )
        .await;
    }

    for (name, proof) in [
        (
            "close/wrong-repayment-id",
            B256::repeat_byte(0xa1).abi_encode(),
        ),
        ("close/non-32-byte-proof", vec![0xa2; 31]),
    ] {
        let outcome = stage_case(RemoteCommand::CloseDestination, |_, _, order_id| {
            close_step(order_id, proof.into())
        })
        .await;
        assert_rejected(name, outcome, &mut accepted).await;
    }

    assert!(
        accepted.is_empty(),
        "authenticated staging accepted immutable proof plans that cannot satisfy the contracts: {accepted:?}",
    );
}
