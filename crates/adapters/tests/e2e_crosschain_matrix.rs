//! Table-driven backend matrix across chain direction, destination asset form, and route.

use alloy::primitives::{keccak256, Address, Bytes, Uint, B256, U256};
use alloy::{
    sol,
    sol_types::{eip712_domain, SolCall, SolStruct, SolValue},
};
use async_trait::async_trait;
use axum::http::HeaderValue;
use solvent_adapters::{
    crosschain::{
        AlloyStepValidator, SolventClient, SqliteLegQuoteStore, SqlitePreparationStore,
        SqliteSagaStore, SqliteStepStore,
    },
    http::crosschain_internal_router,
    ledger::SqliteLedgerStore,
};
use solvent_core::{
    crosschain::{CrossChainProxy, LocalCrossChainService, LocalStepService},
    deps::{
        crosschain::{
            CctpCompletion, CctpCompletionError, CctpPreparedStep, CctpQuoteTerms, LegQuoter,
            LegQuoterError, RemoteSolvent, SagaStore,
        },
        execution::{Execution, ExecutionError, SimError, SimGate},
        ledger::{BudgetSource, BudgetSourceError, Clock},
    },
    ledger::LedgerService,
    primitives::{
        crosschain::{
            AggregateQuote, ChainExecutionPlan, CrossChainRoute, LegQuote, LegQuoteRequest,
            LegRole, PreparedStep, RemoteCommand, SagaState,
        },
        execution::{ExecHandle, ExecStatus, FillTx, SimVerdict, TrackedFill},
        ledger::{AccountKey, ReservationSource},
        ChainId, CrossChainOrderId, CrossChainStepId, IntentId, MakerId, ReservationId,
        StrategyHash,
    },
};
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::Mutex;

const NOW: u64 = 1_700_000_000;
const C1: ChainId = ChainId(11_155_111);
const C2: ChainId = ChainId(84_532);
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
    interface Dest { function fillDirect(SolventCrossChainOrder order, SolventMandate mandate, DirectMakerQuote quote, bytes signature); function fillCredit(SolventCrossChainOrder order, SolventMandate mandate, MakerCreditQuote quote, bytes signature); function confirmDirectRepayment(bytes32 orderId, bytes proof); function completeRepayment(bytes32 orderId, bytes message, bytes attestation); }
    interface Origin { function settleDirect(SolventCrossChainOrder order, SolventMandate mandate, bytes proof, Claim claim); function settleRouted(SolventCrossChainOrder order, SolventMandate mandate, bytes proof, Claim claim, SwapOrder makerOrder); }
    interface Outbox { function dispatch(bytes32 orderId, bytes envelope) payable returns (bytes32); }
}

#[derive(Clone, Copy)]
struct Case {
    origin: ChainId,
    destination: ChainId,
    native: bool,
    route: CrossChainRoute,
}
struct FixedClock;
impl Clock for FixedClock {
    fn now_unix(&self) -> u64 {
        NOW
    }
}
struct Budget;
#[async_trait]
impl BudgetSource for Budget {
    async fn budget(&self, _: &AccountKey) -> Result<U256, BudgetSourceError> {
        Ok(U256::MAX)
    }
}

fn maker(c: ChainId) -> Address {
    Address::from([c.0 as u8; 20])
}
fn operator(c: ChainId) -> Address {
    Address::from([(c.0 as u8).wrapping_add(1); 20])
}
fn app(c: ChainId) -> Address {
    Address::from([(c.0 as u8).wrapping_add(2); 20])
}
fn settler(c: ChainId) -> Address {
    Address::from([(c.0 as u8).wrapping_add(3); 20])
}
fn outbox(c: ChainId) -> Address {
    Address::from([(c.0 as u8).wrapping_add(4); 20])
}
fn wrapped(c: ChainId) -> Address {
    Address::from([(c.0 as u8).wrapping_add(5); 20])
}
fn swap(c: ChainId) -> SwapOrder {
    SwapOrder {
        maker: maker(c),
        traits: U256::from(7),
        data: Bytes::from_static(b"matrix"),
    }
}

struct Quoter(ChainId);
#[async_trait]
impl LegQuoter for Quoter {
    async fn quote(&self, r: &LegQuoteRequest) -> Result<LegQuote, LegQuoterError> {
        let amount_out = match (r.role, r.route) {
            (LegRole::Destination, _) => r.amount - U256::from(7),
            (LegRole::Origin, CrossChainRoute::Cctp) => U256::from(903),
            _ => r.amount,
        };
        let sources = match (r.role, r.route) {
            (LegRole::Destination, _) => vec![ReservationSource {
                maker: MakerId(maker(self.0)),
                strategy_hash: StrategyHash(B256::repeat_byte(0x44)),
                token: if r.output_token.is_zero() {
                    wrapped(self.0)
                } else {
                    r.output_token
                },
                amount: amount_out,
            }],
            (LegRole::Origin, CrossChainRoute::Cctp) => vec![ReservationSource {
                maker: MakerId(maker(self.0)),
                strategy_hash: StrategyHash(keccak256(swap(self.0).abi_encode())),
                token: r.output_token,
                amount: amount_out,
            }],
            _ => Vec::new(),
        };
        let mut q = LegQuote {
            quote_id: B256::ZERO,
            request_id: r.request_id,
            role: r.role,
            local_chain: r.local_chain,
            remote_chain: r.remote_chain,
            input_token: r.input_token,
            output_token: r.output_token,
            amount_in: r.amount,
            amount_out,
            route: r.route,
            price_impact_bps: None,
            block_number: self.0 .0,
            expires_at_unix: NOW + 300,
            sources,
        };
        q.quote_id = solvent_core::crosschain::leg_quote_id(&q);
        Ok(q)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Sent {
    chain: ChainId,
    intent: IntentId,
    target: Address,
}
struct Exec {
    chain: ChainId,
    statuses: Mutex<BTreeMap<B256, ExecStatus>>,
    sent: Arc<Mutex<Vec<Sent>>>,
}
#[async_trait]
impl Execution for Exec {
    async fn submit(&self, fill: &FillTx, _: ReservationId) -> Result<ExecHandle, ExecutionError> {
        self.sent.lock().await.push(Sent {
            chain: self.chain,
            intent: fill.intent,
            target: fill.filler,
        });
        self.statuses
            .lock()
            .await
            .insert(fill.intent.0, ExecStatus::Pending);
        Ok(ExecHandle(fill.intent.0))
    }
    async fn status(&self, h: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError> {
        Ok(self.statuses.lock().await.get(&h.0).cloned())
    }
    async fn forget(&self, i: IntentId) -> Result<(), ExecutionError> {
        self.statuses.lock().await.remove(&i.0);
        Ok(())
    }
    async fn tracked(&self) -> Result<Vec<TrackedFill>, ExecutionError> {
        Ok(Vec::new())
    }
    async fn tick(&self) -> Result<(), ExecutionError> {
        for (id, status) in self.statuses.lock().await.iter_mut() {
            *status = ExecStatus::Confirmed {
                block: self.chain.0,
                tx: *id,
            };
        }
        Ok(())
    }
}
struct Sim;
#[async_trait]
impl SimGate for Sim {
    async fn simulate(&self, _: &FillTx) -> Result<SimVerdict, SimError> {
        Ok(SimVerdict::Ok)
    }
}

struct Chain {
    pool: SqlitePool,
    url: String,
    _server: tokio::task::JoinHandle<()>,
}
async fn pool() -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap()
}
async fn chain(chain: ChainId, role: LegRole, sent: Arc<Mutex<Vec<Sent>>>) -> Chain {
    let pool = pool().await;
    let preparations = Arc::new(SqlitePreparationStore::new(pool.clone()));
    preparations.migrate().await.unwrap();
    let local = Arc::new(LocalCrossChainService::new(
        chain,
        Arc::new(Quoter(chain)),
        Arc::new(SqliteLegQuoteStore::new(pool.clone())),
        preparations,
        Arc::new(LedgerService::new(
            Arc::new(SqliteLedgerStore::new(pool.clone())),
            Arc::new(Budget),
            Arc::new(FixedClock),
        )),
        Arc::new(FixedClock),
    ));
    let commands = match role {
        LegRole::Origin => vec![
            (RemoteCommand::ClaimOrigin, settler(chain)),
            (RemoteCommand::DispatchRepayment, outbox(chain)),
        ],
        LegRole::Destination => vec![
            (RemoteCommand::Deliver, app(chain)),
            (RemoteCommand::DispatchFillProof, outbox(chain)),
            (RemoteCommand::CloseDestination, app(chain)),
        ],
    };
    let steps = Arc::new(
        LocalStepService::new(
            chain,
            operator(chain),
            commands.into_iter().collect(),
            Arc::new(SqliteStepStore::new(pool.clone())),
            Arc::new(Exec {
                chain,
                statuses: Mutex::new(BTreeMap::new()),
                sent,
            }),
            Arc::new(Sim),
        )
        .with_validator(Arc::new(
            AlloyStepValidator::new(operator(chain), wrapped(chain)).with_applications(
                settler(if role == LegRole::Origin {
                    chain
                } else if chain == C1 {
                    C2
                } else {
                    C1
                }),
                app(if role == LegRole::Destination {
                    chain
                } else if chain == C1 {
                    C2
                } else {
                    C1
                }),
            ),
        )),
    );
    let router =
        crosschain_internal_router(local, steps, HeaderValue::from_static("Bearer matrix"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Chain {
        pool,
        url: format!("http://{address}/"),
        _server: server,
    }
}

struct Cctp {
    destination: ChainId,
}
#[async_trait]
impl CctpCompletion for Cctp {
    async fn quote_terms(&self, _: U256) -> Result<CctpQuoteTerms, CctpCompletionError> {
        Ok(CctpQuoteTerms {
            max_fee: U256::from(3),
            finality_threshold: 2_000,
        })
    }
    async fn close_step(
        &self,
        order: CrossChainOrderId,
        _: B256,
    ) -> Result<CctpPreparedStep, CctpCompletionError> {
        Ok(CctpPreparedStep {
            step: PreparedStep {
                command: RemoteCommand::CloseDestination,
                target: app(self.destination),
                value: U256::ZERO,
                calldata: Dest::completeRepaymentCall {
                    orderId: order.0,
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
    case: Case,
    quote: &AggregateQuote,
) -> (CrossChainOrderId, ChainExecutionPlan, ChainExecutionPlan) {
    let route_kind = if case.route == CrossChainRoute::Direct {
        1
    } else {
        0
    };
    let order = SolventCrossChainOrder {
        user: Address::repeat_byte(0x80),
        nonce: U256::from(9),
        originChainId: U256::from(case.origin.0),
        originSettler: settler(case.origin),
        compact: Address::repeat_byte(0x81),
        compactId: U256::from(2),
        compactExpires: U256::from(NOW + 200),
        inputToken: quote.origin.input_token,
        inputAmount: quote.amount_in,
        destinationChainId: U256::from(case.destination.0),
        outputToken: quote.destination.output_token,
        minimumOutputAmount: quote.amount_out,
        recipient: Address::repeat_byte(0x82),
        destinationSettler: app(case.destination),
        fillProofVerifier: Address::repeat_byte(0x83),
        exclusiveFiller: operator(case.destination),
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
        chain_id: case.destination.0,
        verifying_contract: app(case.destination),
    };
    let (deliver, origin_strategy, maker_quote_hash, repayment_token, repayment_amount) =
        match case.route {
            CrossChainRoute::Direct => {
                let maker_quote = DirectMakerQuote {
                    orderId: order_id.0,
                    maker: maker(case.destination),
                    destinationStrategyHash: B256::repeat_byte(0x44),
                    originStrategyHash: B256::repeat_byte(0x45),
                    outputAmount: quote.amount_out,
                    repaymentAmount: quote.amount_in,
                    nonce: U256::from(3),
                    expires: U48::from(NOW + 100),
                };
                let digest = maker_quote.eip712_signing_hash(&domain);
                (
                    Dest::fillDirectCall {
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
                    maker: maker(case.destination),
                    destinationStrategyHash: B256::repeat_byte(0x44),
                    outputAmount: quote.amount_out,
                    usdcDue: quote.destination.amount_in,
                    maxCctpFee: quote.bridge_fee,
                    nonce: U256::from(3),
                    expires: U48::from(NOW + 100),
                };
                let digest = maker_quote.eip712_signing_hash(&domain);
                (
                    Dest::fillCreditCall {
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
    let fill_id = proof_id(case.destination, app(case.destination), order_id, 0);
    let repayment_id = proof_id(case.origin, settler(case.origin), order_id, 1);
    let fill_envelope: Bytes = ProofEnvelope {
        version: 1,
        kind: 0,
        payload: VerifiedFill {
            orderId: order_id.0,
            routeKind: route_kind,
            destinationChainId: U256::from(case.destination.0),
            destinationApp: app(case.destination),
            recipient: order.recipient,
            outputToken: quote.destination.output_token,
            outputAmount: quote.amount_out,
            destinationMaker: maker(case.destination),
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
            originChainId: U256::from(case.origin.0),
            originSettler: settler(case.origin),
            maker: maker(case.destination),
            repaymentToken: quote.origin.input_token,
            repaymentAmount: quote.amount_in,
            repaymentId: repayment_id,
        }
        .abi_encode()
        .into(),
    }
    .abi_encode()
    .into();
    let claim_origin = match case.route {
        CrossChainRoute::Direct => Origin::settleDirectCall {
            order: order.clone(),
            mandate: mandate.clone(),
            proof: fill_id.abi_encode().into(),
            claim: claim(),
        }
        .abi_encode(),
        CrossChainRoute::Cctp => Origin::settleRoutedCall {
            order: order.clone(),
            mandate: mandate.clone(),
            proof: fill_id.abi_encode().into(),
            claim: claim(),
            makerOrder: swap(case.origin),
        }
        .abi_encode(),
    };
    let origin_commands = match case.route {
        CrossChainRoute::Direct => vec![
            PreparedStep {
                command: RemoteCommand::ClaimOrigin,
                target: settler(case.origin),
                value: U256::ZERO,
                calldata: claim_origin.into(),
            },
            PreparedStep {
                command: RemoteCommand::DispatchRepayment,
                target: outbox(case.origin),
                value: U256::ZERO,
                calldata: Outbox::dispatchCall {
                    orderId: order_id.0,
                    envelope: repayment_envelope,
                }
                .abi_encode()
                .into(),
            },
        ],
        CrossChainRoute::Cctp => vec![PreparedStep {
            command: RemoteCommand::ClaimOrigin,
            target: settler(case.origin),
            value: U256::ZERO,
            calldata: claim_origin.into(),
        }],
    };
    let mut destination_commands = vec![
        PreparedStep {
            command: RemoteCommand::Deliver,
            target: app(case.destination),
            value: U256::ZERO,
            calldata: deliver.into(),
        },
        PreparedStep {
            command: RemoteCommand::DispatchFillProof,
            target: outbox(case.destination),
            value: U256::ZERO,
            calldata: Outbox::dispatchCall {
                orderId: order_id.0,
                envelope: fill_envelope,
            }
            .abi_encode()
            .into(),
        },
    ];
    if case.route == CrossChainRoute::Direct {
        destination_commands.push(PreparedStep {
            command: RemoteCommand::CloseDestination,
            target: app(case.destination),
            value: U256::ZERO,
            calldata: Dest::confirmDirectRepaymentCall {
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
            chain_id: case.origin,
            steps: origin_commands,
        },
        ChainExecutionPlan {
            aggregate_id: quote.id,
            chain_id: case.destination,
            steps: destination_commands,
        },
    )
}

fn step_id(order: CrossChainOrderId, chain: ChainId, command: RemoteCommand) -> CrossChainStepId {
    let mut bytes = Vec::with_capacity(41);
    bytes.extend_from_slice(order.0.as_slice());
    bytes.extend_from_slice(&chain.0.to_be_bytes());
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

async fn stored_count(pool: &SqlitePool, table: &str, state: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE state = ?"))
        .bind(state)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn supported_crosschain_scenario_matrix() {
    let cases = [
        Case {
            origin: C1,
            destination: C2,
            native: false,
            route: CrossChainRoute::Direct,
        },
        Case {
            origin: C1,
            destination: C2,
            native: true,
            route: CrossChainRoute::Direct,
        },
        Case {
            origin: C2,
            destination: C1,
            native: false,
            route: CrossChainRoute::Direct,
        },
        Case {
            origin: C2,
            destination: C1,
            native: true,
            route: CrossChainRoute::Direct,
        },
        Case {
            origin: C1,
            destination: C2,
            native: false,
            route: CrossChainRoute::Cctp,
        },
        Case {
            origin: C1,
            destination: C2,
            native: true,
            route: CrossChainRoute::Cctp,
        },
        Case {
            origin: C2,
            destination: C1,
            native: false,
            route: CrossChainRoute::Cctp,
        },
        Case {
            origin: C2,
            destination: C1,
            native: true,
            route: CrossChainRoute::Cctp,
        },
    ];
    for (index, case) in cases.into_iter().enumerate() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let origin = chain(case.origin, LegRole::Origin, Arc::clone(&sent)).await;
        let destination = chain(case.destination, LegRole::Destination, Arc::clone(&sent)).await;
        let saga_pool = pool().await;
        let saga_store = Arc::new(SqliteSagaStore::new(saga_pool));
        saga_store.migrate().await.unwrap();
        let origin_remote: Arc<dyn RemoteSolvent> =
            Arc::new(SolventClient::from_base_url(&origin.url, "matrix").unwrap());
        let destination_remote: Arc<dyn RemoteSolvent> =
            Arc::new(SolventClient::from_base_url(&destination.url, "matrix").unwrap());
        let mut proxy = CrossChainProxy::new(origin_remote, destination_remote, saga_store.clone());
        if case.route == CrossChainRoute::Cctp {
            proxy = proxy.with_cctp(Arc::new(Cctp {
                destination: case.destination,
            }));
        }
        let proxy = Arc::new(proxy);
        let output = if case.native {
            Address::ZERO
        } else if case.destination == C2 {
            Address::repeat_byte(0xb2)
        } else {
            Address::repeat_byte(0xa1)
        };
        let request = B256::from([index as u8 + 1; 32]);
        let quote = proxy
            .quote(
                &LegQuoteRequest {
                    request_id: request,
                    role: LegRole::Origin,
                    local_chain: case.origin,
                    remote_chain: case.destination,
                    input_token: if case.origin == C1 {
                        Address::repeat_byte(0xa1)
                    } else {
                        Address::repeat_byte(0xb2)
                    },
                    output_token: Address::repeat_byte(0xc1),
                    amount: U256::from(1_000),
                    deadline_unix: NOW + 300,
                    route: case.route,
                },
                &LegQuoteRequest {
                    request_id: request,
                    role: LegRole::Destination,
                    local_chain: case.destination,
                    remote_chain: case.origin,
                    input_token: Address::repeat_byte(0xc2),
                    output_token: output,
                    amount: U256::from(900),
                    deadline_unix: NOW + 300,
                    route: case.route,
                },
                NOW,
            )
            .await
            .unwrap();
        assert_eq!(quote.destination.output_token.is_zero(), case.native);
        let (order, origin_plan, destination_plan) = plans(case, &quote);
        let mut saga = proxy
            .start(order, quote.clone(), &origin_plan, &destination_plan, NOW)
            .await
            .unwrap();
        assert_eq!(saga.state, SagaState::Prepared);
        let mut states = Vec::new();
        for advance in 0..5 {
            saga = proxy.advance(order, NOW + advance + 1).await.unwrap();
            states.push(saga.state);
            assert_eq!(saga_store.load(order).await.unwrap(), Some(saga.clone()));
            if case.route == CrossChainRoute::Cctp {
                let destination_steps: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM crosschain_step")
                        .fetch_one(&destination.pool)
                        .await
                        .unwrap();
                assert_eq!(destination_steps, if advance < 3 { 2 } else { 3 });
            }
        }
        assert_eq!(
            states,
            vec![
                SagaState::DestinationFinalized,
                SagaState::OriginPending,
                SagaState::OriginFinalized,
                SagaState::RepaymentPending,
                SagaState::Complete
            ]
        );
        assert_eq!(
            stored_count(&origin.pool, "crosschain_preparation", "executed").await,
            1
        );
        assert_eq!(
            stored_count(&destination.pool, "crosschain_preparation", "executed").await,
            1
        );
        assert_eq!(
            stored_count(&origin.pool, "ledger_reservation", "posted").await,
            1
        );
        assert_eq!(
            stored_count(&destination.pool, "ledger_reservation", "posted").await,
            1
        );
        let expected = if case.route == CrossChainRoute::Direct {
            vec![
                (case.destination, RemoteCommand::Deliver),
                (case.destination, RemoteCommand::DispatchFillProof),
                (case.origin, RemoteCommand::ClaimOrigin),
                (case.origin, RemoteCommand::DispatchRepayment),
                (case.destination, RemoteCommand::CloseDestination),
            ]
        } else {
            vec![
                (case.destination, RemoteCommand::Deliver),
                (case.destination, RemoteCommand::DispatchFillProof),
                (case.origin, RemoteCommand::ClaimOrigin),
                (case.destination, RemoteCommand::CloseDestination),
            ]
        };
        let actual = sent.lock().await.clone();
        assert_eq!(actual.len(), expected.len());
        for (submission, (chain, command)) in actual.iter().zip(expected) {
            assert_eq!(submission.chain, chain);
            assert_eq!(
                submission.intent,
                IntentId(step_id(order, chain, command).0)
            );
            let expected_target = match command {
                RemoteCommand::Deliver | RemoteCommand::CloseDestination => app(chain),
                RemoteCommand::ClaimOrigin => settler(chain),
                _ => outbox(chain),
            };
            assert_eq!(submission.target, expected_target);
        }
        if case.route == CrossChainRoute::Cctp {
            let staged: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM crosschain_step")
                .fetch_one(&destination.pool)
                .await
                .unwrap();
            assert_eq!(staged, 3);
            assert_eq!(
                saga.repayment.as_ref().and_then(|e| e.message_id),
                Some(B256::repeat_byte(0xcc))
            );
        }
        let replay = proxy
            .start(order, quote, &origin_plan, &destination_plan, NOW)
            .await
            .unwrap();
        assert_eq!(replay, saga);
        assert_eq!(proxy.advance(order, NOW + 6).await.unwrap(), saga);
        assert_eq!(sent.lock().await.len(), actual.len());
    }
}
