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
const ORIGIN_TOKEN: Address = Address::repeat_byte(0x21);
const ORIGIN_USDC: Address = Address::repeat_byte(0x22);
const DESTINATION_USDC: Address = Address::repeat_byte(0x23);
const WRAPPED_NATIVE: Address = Address::repeat_byte(0x24);
const DESTINATION_APP: Address = Address::repeat_byte(0x31);
const ORIGIN_SETTLER: Address = Address::repeat_byte(0x32);
const DESTINATION_OUTBOX: Address = Address::repeat_byte(0x34);
const OPERATOR: Address = Address::repeat_byte(0x33);
const DESTINATION_MAKER: Address = Address::repeat_byte(0x41);
const ORIGIN_MAKER: Address = Address::repeat_byte(0x42);
const DESTINATION_STRATEGY: B256 = B256::repeat_byte(0x51);

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

    struct MakerCreditQuote {
        bytes32 orderId;
        address maker;
        bytes32 destinationStrategyHash;
        uint256 outputAmount;
        uint256 usdcDue;
        uint256 maxCctpFee;
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

    struct SwapOrder {
        address maker;
        uint256 traits;
        bytes data;
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

    interface DestinationApp {
        function fillCredit(
            SolventCrossChainOrder order,
            SolventMandate mandate,
            MakerCreditQuote quote,
            bytes makerSignature
        ) external;
    }

    interface OriginSettler {
        function settleRouted(
            SolventCrossChainOrder order,
            SolventMandate mandate,
            bytes fillProof,
            Claim compactClaim,
            SwapOrder makerOrder
        ) external;
    }

    interface ProofOutbox {
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
    amount_out: U256,
    sources: Vec<ReservationSource>,
}

#[async_trait]
impl LegQuoter for FixedQuoter {
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
            amount_out: self.amount_out,
            route: request.route,
            price_impact_bps: None,
            block_number: 100,
            expires_at_unix: NOW + 60,
            sources: self.sources.clone(),
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

#[derive(Clone, Copy, Debug)]
enum DestinationCase {
    ValidNative,
    WrongMaker,
    WrongStrategy,
    WrongWrappedToken,
    WrongAmount,
    WrongQuoteAmount,
    WrongPlanChain,
    WrongTarget,
    WrongOrder,
}

#[derive(Clone, Copy, Debug)]
enum OriginCase {
    Valid,
    WrongMaker,
    WrongStrategy,
    WrongToken,
    WrongAmount,
}

struct StageOutcome {
    status: StatusCode,
    persisted: bool,
    bound: bool,
    simulations: usize,
    submissions: usize,
}

fn remote_quote(
    role: LegRole,
    request_id: B256,
    output_token: Address,
    amount_in: U256,
    amount_out: U256,
    sources: Vec<ReservationSource>,
) -> LegQuote {
    let (local_chain, remote_chain, input_token) = match role {
        LegRole::Origin => (ORIGIN_CHAIN, DESTINATION_CHAIN, ORIGIN_TOKEN),
        LegRole::Destination => (DESTINATION_CHAIN, ORIGIN_CHAIN, DESTINATION_USDC),
    };
    let mut quote = LegQuote {
        quote_id: B256::ZERO,
        request_id,
        role,
        local_chain,
        remote_chain,
        input_token,
        output_token,
        amount_in,
        amount_out,
        route: CrossChainRoute::Cctp,
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
        user: Address::repeat_byte(0x61),
        nonce: U256::from(1),
        originChainId: U256::from(quote.origin.local_chain.0),
        originSettler: ORIGIN_SETTLER,
        compact: Address::repeat_byte(0x62),
        compactId: U256::from(2),
        compactExpires: U256::from(NOW + 60),
        inputToken: quote.origin.input_token,
        inputAmount: quote.amount_in,
        destinationChainId: U256::from(quote.destination.local_chain.0),
        outputToken: quote.destination.output_token,
        minimumOutputAmount: quote.amount_out,
        recipient: Address::repeat_byte(0x63),
        destinationSettler: DESTINATION_APP,
        fillProofVerifier: Address::repeat_byte(0x64),
        exclusiveFiller: OPERATOR,
        exclusivityEnds: U48::from(NOW + 30),
        fillDeadline: U48::from(NOW + 50),
        routeKind: 0,
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

fn origin_maker_order() -> SwapOrder {
    SwapOrder {
        maker: ORIGIN_MAKER,
        traits: U256::from(7),
        data: Bytes::from_static(b"bound-origin-swap"),
    }
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

fn credit_quote(quote: &AggregateQuote, order_id: CrossChainOrderId) -> MakerCreditQuote {
    MakerCreditQuote {
        orderId: order_id.0,
        maker: DESTINATION_MAKER,
        destinationStrategyHash: DESTINATION_STRATEGY,
        outputAmount: quote.amount_out,
        usdcDue: quote.destination.amount_in,
        maxCctpFee: quote.bridge_fee,
        nonce: U256::from(3),
        expires: U48::from(NOW + 50),
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

fn valid_fill_dispatch(
    quote: &AggregateQuote,
    settlement_order: &SolventCrossChainOrder,
    order_id: CrossChainOrderId,
) -> PreparedStep {
    let maker_quote = credit_quote(quote, order_id);
    let domain = eip712_domain! {
        name: "Solvent Cross-Chain Aqua App",
        version: "1",
        chain_id: DESTINATION_CHAIN.0,
        verifying_contract: DESTINATION_APP,
    };
    let payload = VerifiedFill {
        orderId: order_id.0,
        routeKind: 0,
        destinationChainId: U256::from(DESTINATION_CHAIN.0),
        destinationApp: DESTINATION_APP,
        recipient: settlement_order.recipient,
        outputToken: quote.destination.output_token,
        outputAmount: quote.amount_out,
        destinationMaker: DESTINATION_MAKER,
        destinationStrategyHash: DESTINATION_STRATEGY,
        originStrategyHash: B256::ZERO,
        repaymentToken: quote.destination.input_token,
        repaymentAmount: quote.destination.amount_in,
        maxCctpFee: quote.bridge_fee,
        makerQuoteHash: maker_quote.eip712_signing_hash(&domain),
        fillId: fill_id(order_id),
    }
    .abi_encode();
    let envelope = ProofEnvelope {
        version: 1,
        kind: 0,
        payload: payload.into(),
    }
    .abi_encode();
    PreparedStep {
        command: RemoteCommand::DispatchFillProof,
        target: DESTINATION_OUTBOX,
        value: U256::ZERO,
        calldata: ProofOutbox::dispatchCall {
            orderId: order_id.0,
            envelope: envelope.into(),
        }
        .abi_encode()
        .into(),
    }
}

struct StageFixture {
    local_chain: ChainId,
    local_request: LegQuoteRequest,
    local_amount_out: U256,
    local_sources: Vec<ReservationSource>,
    remote: LegQuote,
    context_role: LegRole,
    plan_chain: ChainId,
}

async fn stage_case(
    fixture: StageFixture,
    make_step: impl FnOnce(&AggregateQuote, CrossChainOrderId) -> PreparedStep,
) -> StageOutcome {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open verifier SQLite");
    let preparations = Arc::new(SqlitePreparationStore::new(pool.clone()));
    preparations.migrate().await.expect("apply schema");
    let clock: Arc<dyn Clock> = Arc::new(FixedClock);
    let local = Arc::new(LocalCrossChainService::new(
        fixture.local_chain,
        Arc::new(FixedQuoter {
            amount_out: fixture.local_amount_out,
            sources: fixture.local_sources,
        }),
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
        .quote(&fixture.local_request)
        .await
        .expect("issue local quote");
    let quote = match fixture.context_role {
        LegRole::Origin => aggregate_quote(local_quote, fixture.remote, NOW),
        LegRole::Destination => aggregate_quote(fixture.remote, local_quote, NOW),
    }
    .expect("aggregate CCTP quote");
    let quote = bind_cctp_terms(quote);
    let initial_order = order(&quote);
    let order_id = CrossChainOrderId(initial_order.eip712_hash_struct());
    let step = make_step(&quote, order_id);
    let command = step.command;
    let plan_steps = match fixture.context_role {
        LegRole::Origin => vec![step],
        LegRole::Destination => vec![step, valid_fill_dispatch(&quote, &initial_order, order_id)],
    };
    let targets = match fixture.context_role {
        LegRole::Origin => BTreeMap::from([(RemoteCommand::ClaimOrigin, ORIGIN_SETTLER)]),
        LegRole::Destination => BTreeMap::from([
            (RemoteCommand::Deliver, DESTINATION_APP),
            (RemoteCommand::DispatchFillProof, DESTINATION_OUTBOX),
        ]),
    };
    let plan = ChainExecutionPlan {
        aggregate_id: quote.id,
        chain_id: fixture.plan_chain,
        steps: plan_steps,
    };
    let context = StepValidationContext {
        order_id,
        quote,
        role: fixture.context_role,
    };
    let store = Arc::new(SqliteStepStore::new(pool));
    let probe = Arc::new(ActionProbe::default());
    let steps = Arc::new(
        LocalStepService::new(
            fixture.local_chain,
            OPERATOR,
            targets,
            store.clone(),
            probe.clone(),
            probe.clone(),
        )
        .with_validator(Arc::new(
            AlloyStepValidator::new(OPERATOR, WRAPPED_NATIVE)
                .with_applications(ORIGIN_SETTLER, DESTINATION_APP),
        )),
    );
    let router = crosschain_internal_router(
        local,
        steps,
        HeaderValue::from_static("Bearer verifier-cctp"),
    );
    let body = serde_json::to_vec(&serde_json::json!({
        "context": context,
        "plan": plan,
    }))
    .expect("serialize stage request");
    let response = router
        .oneshot(
            Request::post("/internal/v1/cross-chain/stage")
                .header("authorization", "Bearer verifier-cctp")
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

async fn destination_case(case: DestinationCase) -> StageOutcome {
    let request_id = B256::repeat_byte(0x71);
    let (source_maker, source_strategy, source_token, source_amount) = match case {
        DestinationCase::WrongWrappedToken => (
            DESTINATION_MAKER,
            DESTINATION_STRATEGY,
            Address::repeat_byte(0x25),
            U256::from(850),
        ),
        DestinationCase::WrongAmount => (
            DESTINATION_MAKER,
            DESTINATION_STRATEGY,
            WRAPPED_NATIVE,
            U256::from(849),
        ),
        _ => (
            DESTINATION_MAKER,
            DESTINATION_STRATEGY,
            WRAPPED_NATIVE,
            U256::from(850),
        ),
    };
    let destination_request = LegQuoteRequest {
        request_id,
        role: LegRole::Destination,
        local_chain: DESTINATION_CHAIN,
        remote_chain: ORIGIN_CHAIN,
        input_token: DESTINATION_USDC,
        output_token: Address::ZERO,
        amount: U256::from(900),
        deadline_unix: NOW + 60,
        route: CrossChainRoute::Cctp,
    };
    let origin = remote_quote(
        LegRole::Origin,
        request_id,
        ORIGIN_USDC,
        U256::from(1_000),
        U256::from(903),
        Vec::new(),
    );
    let plan_chain = if matches!(case, DestinationCase::WrongPlanChain) {
        ORIGIN_CHAIN
    } else {
        DESTINATION_CHAIN
    };
    stage_case(
        StageFixture {
            local_chain: DESTINATION_CHAIN,
            local_request: destination_request,
            local_amount_out: U256::from(850),
            local_sources: vec![ReservationSource {
                maker: MakerId(source_maker),
                strategy_hash: StrategyHash(source_strategy),
                token: source_token,
                amount: source_amount,
            }],
            remote: origin,
            context_role: LegRole::Destination,
            plan_chain,
        },
        |quote, _order_id| {
            let mut settlement_order = order(quote);
            if matches!(case, DestinationCase::WrongOrder) {
                settlement_order.nonce = U256::from(999);
            }
            let embedded_order_id = CrossChainOrderId(settlement_order.eip712_hash_struct());
            let signed_maker = if matches!(case, DestinationCase::WrongMaker) {
                Address::repeat_byte(0x43)
            } else {
                DESTINATION_MAKER
            };
            let signed_strategy = if matches!(case, DestinationCase::WrongStrategy) {
                B256::repeat_byte(0x52)
            } else {
                DESTINATION_STRATEGY
            };
            let signed_output = if matches!(case, DestinationCase::WrongQuoteAmount) {
                U256::from(849)
            } else {
                quote.amount_out
            };
            let target = if matches!(case, DestinationCase::WrongTarget) {
                Address::repeat_byte(0x34)
            } else {
                DESTINATION_APP
            };
            PreparedStep {
                command: RemoteCommand::Deliver,
                target,
                value: U256::ZERO,
                calldata: DestinationApp::fillCreditCall {
                    order: settlement_order.clone(),
                    mandate: mandate(&settlement_order, embedded_order_id),
                    quote: MakerCreditQuote {
                        orderId: embedded_order_id.0,
                        maker: signed_maker,
                        destinationStrategyHash: signed_strategy,
                        outputAmount: signed_output,
                        usdcDue: quote.destination.amount_in,
                        maxCctpFee: quote.bridge_fee,
                        nonce: U256::from(3),
                        expires: U48::from(NOW + 50),
                    },
                    makerSignature: Bytes::from_static(b"signed-credit-quote"),
                }
                .abi_encode()
                .into(),
            }
        },
    )
    .await
}

async fn origin_case(case: OriginCase) -> StageOutcome {
    let request_id = B256::repeat_byte(0x72);
    let maker_order = origin_maker_order();
    let (source_token, source_amount) = match case {
        OriginCase::WrongToken => (Address::repeat_byte(0x26), U256::from(903)),
        OriginCase::WrongAmount => (ORIGIN_USDC, U256::from(902)),
        _ => (ORIGIN_USDC, U256::from(903)),
    };
    let origin_request = LegQuoteRequest {
        request_id,
        role: LegRole::Origin,
        local_chain: ORIGIN_CHAIN,
        remote_chain: DESTINATION_CHAIN,
        input_token: ORIGIN_TOKEN,
        output_token: ORIGIN_USDC,
        amount: U256::from(1_000),
        deadline_unix: NOW + 60,
        route: CrossChainRoute::Cctp,
    };
    let destination = remote_quote(
        LegRole::Destination,
        request_id,
        Address::repeat_byte(0x27),
        U256::from(900),
        U256::from(850),
        Vec::new(),
    );
    stage_case(
        StageFixture {
            local_chain: ORIGIN_CHAIN,
            local_request: origin_request,
            local_amount_out: U256::from(903),
            local_sources: vec![ReservationSource {
                maker: MakerId(ORIGIN_MAKER),
                strategy_hash: StrategyHash(keccak256(maker_order.abi_encode())),
                token: source_token,
                amount: source_amount,
            }],
            remote: destination,
            context_role: LegRole::Origin,
            plan_chain: ORIGIN_CHAIN,
        },
        |quote, order_id| {
            let settlement_order = order(quote);
            let mut submitted_maker_order = origin_maker_order();
            if matches!(case, OriginCase::WrongMaker) {
                submitted_maker_order.maker = Address::repeat_byte(0x43);
            }
            if matches!(case, OriginCase::WrongStrategy) {
                submitted_maker_order.data = Bytes::from_static(b"substituted-origin-swap");
            }
            PreparedStep {
                command: RemoteCommand::ClaimOrigin,
                target: ORIGIN_SETTLER,
                value: U256::ZERO,
                calldata: OriginSettler::settleRoutedCall {
                    order: settlement_order.clone(),
                    mandate: mandate(&settlement_order, order_id),
                    fillProof: fill_id(order_id).abi_encode().into(),
                    compactClaim: Claim {
                        allocatorData: Bytes::new(),
                        sponsorSignature: Bytes::new(),
                        sponsor: settlement_order.user,
                        nonce: U256::from(1),
                        expires: U256::from(NOW + 50),
                        witness: B256::ZERO,
                        witnessTypestring: String::new(),
                        id: U256::from(1),
                        allocatedAmount: quote.amount_in,
                        claimants: Vec::new(),
                    },
                    makerOrder: submitted_maker_order,
                }
                .abi_encode()
                .into(),
            }
        },
    )
    .await
}

fn assert_rejected(case: impl std::fmt::Debug, outcome: StageOutcome) {
    assert_eq!(
        (
            outcome.status,
            outcome.persisted,
            outcome.bound,
            outcome.simulations,
            outcome.submissions,
        ),
        (StatusCode::BAD_REQUEST, false, false, 0, 0),
        "{case:?} must fail before persistence or chain interaction",
    );
}

#[tokio::test]
async fn cctp_destination_binds_every_reserved_source_term_at_the_private_boundary() {
    let valid = destination_case(DestinationCase::ValidNative).await;
    assert_eq!(
        (
            valid.status,
            valid.persisted,
            valid.bound,
            valid.simulations,
            valid.submissions,
        ),
        (StatusCode::OK, true, true, 0, 0),
        "native delivery must accept the configured wrapped-native reserve",
    );

    for case in [
        DestinationCase::WrongMaker,
        DestinationCase::WrongStrategy,
        DestinationCase::WrongWrappedToken,
        DestinationCase::WrongAmount,
        DestinationCase::WrongQuoteAmount,
        DestinationCase::WrongPlanChain,
        DestinationCase::WrongTarget,
        DestinationCase::WrongOrder,
    ] {
        assert_rejected(case, destination_case(case).await);
    }
}

#[tokio::test]
async fn cctp_origin_binds_the_routed_swap_to_the_reserved_source_at_the_private_boundary() {
    let valid = origin_case(OriginCase::Valid).await;
    assert_eq!(
        (
            valid.status,
            valid.persisted,
            valid.bound,
            valid.simulations,
            valid.submissions,
        ),
        (StatusCode::OK, true, true, 0, 0),
        "the exact reserved routed swap must stage",
    );

    for case in [
        OriginCase::WrongMaker,
        OriginCase::WrongStrategy,
        OriginCase::WrongToken,
        OriginCase::WrongAmount,
    ] {
        assert_rejected(case, origin_case(case).await);
    }
}
