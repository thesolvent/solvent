use alloy::primitives::{keccak256, Address, Bytes, Uint, B256, U256};
use alloy::signers::{local::PrivateKeySigner, SignerSync};
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolCall, SolStruct, SolValue};
use async_trait::async_trait;
use solvent_core::deps::crosschain::{DirectPlanAuthor, DirectPlanAuthorError};
use solvent_core::primitives::crosschain::{
    ChainExecutionPlan, DirectExecutionPlans, DirectOrderAuthorization, PreparedStep, RemoteCommand,
};
use solvent_core::primitives::{ChainId, StrategyHash};

type U48 = Uint<48, 1>;

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

    interface ICrossChainAquaApp {
        function fillDirect(
            SolventCrossChainOrder order,
            SolventMandate mandate,
            DirectMakerQuote quote,
            bytes makerSignature
        ) external;
        function confirmDirectRepayment(bytes32 orderId, bytes repaymentProof) external;
    }

    interface ICompactOriginSettler {
        function settleDirect(
            SolventCrossChainOrder order,
            SolventMandate mandate,
            bytes fillProof,
            Claim compactClaim
        ) external;
    }

    interface ICcipProofOutbox {
        function dispatch(bytes32 orderId, bytes envelope) external payable returns (bytes32 messageId);
    }
}

#[derive(Clone)]
pub struct DirectPlanAuthorConfig {
    pub origin_chain: ChainId,
    pub destination_chain: ChainId,
    pub origin_settler: Address,
    pub destination_app: Address,
    pub origin_proof_outbox: Address,
    pub destination_proof_outbox: Address,
    pub origin_strategy_hash: StrategyHash,
}

pub struct AlloyDirectPlanAuthor {
    config: DirectPlanAuthorConfig,
    maker: PrivateKeySigner,
}

impl AlloyDirectPlanAuthor {
    pub fn new(
        config: DirectPlanAuthorConfig,
        maker: PrivateKeySigner,
    ) -> Result<Self, DirectPlanAuthorError> {
        if config.origin_chain == config.destination_chain
            || config.origin_settler.is_zero()
            || config.destination_app.is_zero()
            || config.origin_proof_outbox.is_zero()
            || config.destination_proof_outbox.is_zero()
            || config.origin_strategy_hash.0.is_zero()
        {
            return Err(DirectPlanAuthorError::Invalid(
                "direct plan author configuration is incomplete".to_string(),
            ));
        }
        Ok(Self { config, maker })
    }

    fn author_plans(
        &self,
        authorization: &DirectOrderAuthorization,
    ) -> Result<DirectExecutionPlans, DirectPlanAuthorError> {
        let quote = &authorization.quote;
        if quote.origin.local_chain != self.config.origin_chain
            || quote.destination.local_chain != self.config.destination_chain
            || authorization.order.origin_settler != self.config.origin_settler
            || authorization.order.destination_settler != self.config.destination_app
        {
            return invalid("authorization does not belong to this direct plan author");
        }
        let [source] = quote.destination.sources.as_slice() else {
            return invalid(&format!(
                "this size fills from {} maker positions on the destination chain; cross-chain settlement can carry only one, so try a smaller amount",
                quote.destination.sources.len()
            ));
        };
        if source.maker.0 != self.maker.address() {
            return invalid("quoted destination maker does not match the configured signer");
        }
        let order = order(authorization)?;
        let mandate = mandate(authorization)?;
        let expires = u48(quote.expires_at_unix, "maker quote expiry")?;
        let maker_quote = DirectMakerQuote {
            orderId: authorization.order_id.0,
            maker: self.maker.address(),
            destinationStrategyHash: source.strategy_hash.0,
            originStrategyHash: self.config.origin_strategy_hash.0,
            outputAmount: quote.amount_out,
            repaymentAmount: quote.amount_in,
            nonce: authorization.order.nonce,
            expires,
        };
        let domain = eip712_domain! {
            name: "Solvent Cross-Chain Aqua App",
            version: "1",
            chain_id: self.config.destination_chain.0,
            verifying_contract: self.config.destination_app,
        };
        let maker_quote_hash = maker_quote.eip712_signing_hash(&domain);
        let maker_signature = sign65(&self.maker, maker_quote_hash);
        let fill_id = proof_id(
            self.config.destination_chain,
            self.config.destination_app,
            authorization.order_id.0,
            0,
        );
        let repayment_id = proof_id(
            self.config.origin_chain,
            self.config.origin_settler,
            authorization.order_id.0,
            1,
        );
        let fill = VerifiedFill {
            orderId: authorization.order_id.0,
            routeKind: 1,
            destinationChainId: U256::from(self.config.destination_chain.0),
            destinationApp: self.config.destination_app,
            recipient: authorization.order.recipient,
            outputToken: quote.destination.output_token,
            outputAmount: quote.amount_out,
            destinationMaker: self.maker.address(),
            destinationStrategyHash: source.strategy_hash.0,
            originStrategyHash: self.config.origin_strategy_hash.0,
            repaymentToken: quote.origin.input_token,
            repaymentAmount: quote.amount_in,
            maxCctpFee: U256::ZERO,
            makerQuoteHash: maker_quote_hash,
            fillId: fill_id,
        };
        let repayment = VerifiedRepayment {
            orderId: authorization.order_id.0,
            originChainId: U256::from(self.config.origin_chain.0),
            originSettler: self.config.origin_settler,
            maker: self.maker.address(),
            repaymentToken: quote.origin.input_token,
            repaymentAmount: quote.amount_in,
            repaymentId: repayment_id,
        };
        let fill_envelope = ProofEnvelope {
            version: 1,
            kind: 0,
            payload: fill.abi_encode().into(),
        }
        .abi_encode();
        let repayment_envelope = ProofEnvelope {
            version: 1,
            kind: 1,
            payload: repayment.abi_encode().into(),
        }
        .abi_encode();
        let claim = Claim {
            allocatorData: Bytes::new(),
            sponsorSignature: authorization.compact_claim.sponsor_signature.clone(),
            sponsor: authorization.compact_claim.sponsor,
            nonce: authorization.compact_claim.nonce,
            expires: U256::from(authorization.compact_claim.expires),
            witness: authorization.compact_claim.witness,
            witnessTypestring: authorization.compact_claim.witness_typestring.clone(),
            id: authorization.compact_claim.id,
            allocatedAmount: authorization.compact_claim.allocated_amount,
            claimants: vec![Component {
                claimant: U256::from_be_slice(authorization.compact_claim.claimant.as_slice()),
                amount: authorization.compact_claim.claimant_amount,
            }],
        };

        Ok(DirectExecutionPlans {
            origin: ChainExecutionPlan {
                aggregate_id: quote.id,
                chain_id: self.config.origin_chain,
                steps: vec![
                    PreparedStep {
                        command: RemoteCommand::ClaimOrigin,
                        target: self.config.origin_settler,
                        value: U256::ZERO,
                        calldata: ICompactOriginSettler::settleDirectCall {
                            order: order.clone(),
                            mandate: mandate.clone(),
                            fillProof: Bytes::from(fill_id.as_slice().to_vec()),
                            compactClaim: claim,
                        }
                        .abi_encode()
                        .into(),
                    },
                    PreparedStep {
                        command: RemoteCommand::DispatchRepayment,
                        target: self.config.origin_proof_outbox,
                        value: U256::ZERO,
                        calldata: ICcipProofOutbox::dispatchCall {
                            orderId: authorization.order_id.0,
                            envelope: repayment_envelope.into(),
                        }
                        .abi_encode()
                        .into(),
                    },
                ],
            },
            destination: ChainExecutionPlan {
                aggregate_id: quote.id,
                chain_id: self.config.destination_chain,
                steps: vec![
                    PreparedStep {
                        command: RemoteCommand::Deliver,
                        target: self.config.destination_app,
                        value: U256::ZERO,
                        calldata: ICrossChainAquaApp::fillDirectCall {
                            order,
                            mandate,
                            quote: maker_quote,
                            makerSignature: maker_signature,
                        }
                        .abi_encode()
                        .into(),
                    },
                    PreparedStep {
                        command: RemoteCommand::DispatchFillProof,
                        target: self.config.destination_proof_outbox,
                        value: U256::ZERO,
                        calldata: ICcipProofOutbox::dispatchCall {
                            orderId: authorization.order_id.0,
                            envelope: fill_envelope.into(),
                        }
                        .abi_encode()
                        .into(),
                    },
                    PreparedStep {
                        command: RemoteCommand::CloseDestination,
                        target: self.config.destination_app,
                        value: U256::ZERO,
                        calldata: ICrossChainAquaApp::confirmDirectRepaymentCall {
                            orderId: authorization.order_id.0,
                            repaymentProof: Bytes::from(repayment_id.as_slice().to_vec()),
                        }
                        .abi_encode()
                        .into(),
                    },
                ],
            },
        })
    }
}

#[async_trait]
impl DirectPlanAuthor for AlloyDirectPlanAuthor {
    #[tracing::instrument(skip_all)]
    async fn author(
        &self,
        authorization: &DirectOrderAuthorization,
    ) -> Result<DirectExecutionPlans, DirectPlanAuthorError> {
        self.author_plans(authorization)
    }
}

fn order(
    authorization: &DirectOrderAuthorization,
) -> Result<SolventCrossChainOrder, DirectPlanAuthorError> {
    let value = &authorization.order;
    Ok(SolventCrossChainOrder {
        user: value.user,
        nonce: value.nonce,
        originChainId: U256::from(value.origin_chain_id),
        originSettler: value.origin_settler,
        compact: value.compact,
        compactId: value.compact_id,
        compactExpires: U256::from(value.compact_expires),
        inputToken: value.input_token,
        inputAmount: value.input_amount,
        destinationChainId: U256::from(value.destination_chain_id),
        outputToken: value.output_token,
        minimumOutputAmount: value.minimum_output_amount,
        recipient: value.recipient,
        destinationSettler: value.destination_settler,
        fillProofVerifier: value.fill_proof_verifier,
        exclusiveFiller: value.exclusive_filler,
        exclusivityEnds: u48(value.exclusivity_ends, "exclusivity end")?,
        fillDeadline: u48(value.fill_deadline, "fill deadline")?,
        routeKind: value.route_kind,
    })
}

fn mandate(
    authorization: &DirectOrderAuthorization,
) -> Result<SolventMandate, DirectPlanAuthorError> {
    let value = &authorization.mandate;
    Ok(SolventMandate {
        orderId: value.order_id,
        destinationChainId: U256::from(value.destination_chain_id),
        destinationSettler: value.destination_settler,
        fillProofVerifier: value.fill_proof_verifier,
        outputToken: value.output_token,
        minimumOutputAmount: value.minimum_output_amount,
        recipient: value.recipient,
        fillDeadline: u48(value.fill_deadline, "mandate fill deadline")?,
        exclusiveFiller: value.exclusive_filler,
        routeKind: value.route_kind,
    })
}

fn proof_id(chain: ChainId, application: Address, order_id: B256, kind: u8) -> B256 {
    keccak256(
        ProofIdMaterial {
            chainId: U256::from(chain.0),
            application,
            orderId: order_id,
            kind,
        }
        .abi_encode(),
    )
}

fn u48(value: u64, name: &str) -> Result<U48, DirectPlanAuthorError> {
    if value >= (1_u64 << 48) {
        return invalid(&format!("{name} exceeds uint48"));
    }
    Ok(U48::from(value))
}

fn invalid<T>(message: &str) -> Result<T, DirectPlanAuthorError> {
    Err(DirectPlanAuthorError::Invalid(message.to_string()))
}

fn sign65(signer: &PrivateKeySigner, digest: B256) -> Bytes {
    let signature = signer
        .sign_hash_sync(&digest)
        .expect("a local signer signs a 32-byte digest infallibly");
    Bytes::from(signature.as_bytes())
}
