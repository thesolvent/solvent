use alloy::primitives::{Address, Bytes, FixedBytes, Signature, Uint, B256, U256};
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolStruct};
use serde::{Deserialize, Serialize};
use solvent_core::crosschain::validate_aggregate_quote;
use solvent_core::primitives::crosschain::{
    AggregateQuote, CompactClaimAuthorization, CrossChainRoute, DirectOrderAuthorization,
    DirectSettlementMandate, DirectSettlementOrder,
};
use solvent_core::primitives::{AggregateQuoteId, ChainId, CrossChainOrderId, SolventError};

type U48 = Uint<48, 1>;

const DIRECT_ROUTE_KIND: u8 = 1;
const MAX_U48: u64 = (1_u64 << 48) - 1;
const MIN_FILL_WINDOW_SECS: u64 = 120;
const MANDATE_WITNESS_TYPESTRING: &str = "bytes32 orderId,uint256 destinationChainId,address destinationSettler,address fillProofVerifier,address outputToken,uint256 minimumOutputAmount,address recipient,uint48 fillDeadline,address exclusiveFiller,uint8 routeKind";

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

    struct Mandate {
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

    struct Compact {
        address arbiter;
        address sponsor;
        uint256 nonce;
        uint256 expires;
        bytes12 lockTag;
        address token;
        uint256 amount;
        Mandate mandate;
    }
}

#[derive(Debug, Clone)]
pub struct DirectOrderDraftConfig {
    pub origin_chain: ChainId,
    pub destination_chain: ChainId,
    pub origin_settler: Address,
    pub compact: Address,
    pub destination_settler: Address,
    pub fill_proof_verifier: Address,
    pub exclusive_filler: Address,
    pub compact_lock_tag: FixedBytes<12>,
}

#[derive(Debug, Clone)]
pub struct DirectOrderDraftBuilder {
    config: DirectOrderDraftConfig,
}

impl DirectOrderDraftBuilder {
    pub fn new(config: DirectOrderDraftConfig) -> Result<Self, SolventError> {
        if config.origin_chain == config.destination_chain
            || config.origin_settler.is_zero()
            || config.compact.is_zero()
            || config.destination_settler.is_zero()
            || config.fill_proof_verifier.is_zero()
            || config.exclusive_filler.is_zero()
        {
            return Err(SolventError::InvalidCrossChain(
                "direct order draft configuration is incomplete".to_string(),
            ));
        }
        Ok(Self { config })
    }

    pub fn draft(
        &self,
        request: DirectOrderDraftRequest,
        now_unix: u64,
    ) -> Result<DirectOrderDraft, SolventError> {
        validate_aggregate_quote(&request.quote, now_unix)?;
        if request.quote.origin.route != CrossChainRoute::Direct
            || request.quote.origin.local_chain != self.config.origin_chain
            || request.quote.destination.local_chain != self.config.destination_chain
        {
            return Err(SolventError::InvalidCrossChain(
                "quote does not belong to the configured direct route".to_string(),
            ));
        }
        if request.sponsor.is_zero() || request.recipient.is_zero() {
            return Err(SolventError::InvalidCrossChain(
                "sponsor and recipient must be non-zero".to_string(),
            ));
        }
        if request.quote.destination.sources.len() != 1 {
            return Err(SolventError::InvalidCrossChain(
                "direct settlement requires exactly one destination maker source".to_string(),
            ));
        }
        let fill_deadline = request.quote.expires_at_unix;
        if fill_deadline < now_unix.saturating_add(MIN_FILL_WINDOW_SECS) {
            return Err(SolventError::InvalidCrossChain(
                "quote expires too soon; request a fresh price".to_string(),
            ));
        }
        if fill_deadline > MAX_U48 || request.compact_expires_unix <= fill_deadline {
            return Err(SolventError::InvalidCrossChain(
                "Compact expiry must be later than the quoted fill deadline".to_string(),
            ));
        }

        let compact_id = compact_id(
            self.config.compact_lock_tag,
            request.quote.origin.input_token,
        );
        let order = SolventCrossChainOrder {
            user: request.sponsor,
            nonce: request.order_nonce,
            originChainId: U256::from(self.config.origin_chain.0),
            originSettler: self.config.origin_settler,
            compact: self.config.compact,
            compactId: compact_id,
            compactExpires: U256::from(request.compact_expires_unix),
            inputToken: request.quote.origin.input_token,
            inputAmount: request.quote.amount_in,
            destinationChainId: U256::from(self.config.destination_chain.0),
            outputToken: request.quote.destination.output_token,
            minimumOutputAmount: request.quote.amount_out,
            recipient: request.recipient,
            destinationSettler: self.config.destination_settler,
            fillProofVerifier: self.config.fill_proof_verifier,
            exclusiveFiller: self.config.exclusive_filler,
            exclusivityEnds: U48::from(fill_deadline),
            fillDeadline: U48::from(fill_deadline),
            routeKind: DIRECT_ROUTE_KIND,
        };
        let order_id = CrossChainOrderId(order.eip712_hash_struct());
        let mandate = SolventCompactMandate {
            order_id: order_id.0,
            destination_chain_id: self.config.destination_chain.0,
            destination_settler: self.config.destination_settler,
            fill_proof_verifier: self.config.fill_proof_verifier,
            output_token: request.quote.destination.output_token,
            minimum_output_amount: request.quote.amount_out,
            recipient: request.recipient,
            fill_deadline,
            exclusive_filler: self.config.exclusive_filler,
            route_kind: DIRECT_ROUTE_KIND,
        };
        Ok(DirectOrderDraft {
            aggregate_id: request.quote.id,
            order_id,
            compact: self.config.compact,
            order: SolventCompactOrder {
                user: request.sponsor,
                nonce: request.order_nonce,
                origin_chain_id: self.config.origin_chain.0,
                origin_settler: self.config.origin_settler,
                compact: self.config.compact,
                compact_id,
                compact_expires: request.compact_expires_unix,
                input_token: request.quote.origin.input_token,
                input_amount: request.quote.amount_in,
                destination_chain_id: self.config.destination_chain.0,
                output_token: request.quote.destination.output_token,
                minimum_output_amount: request.quote.amount_out,
                recipient: request.recipient,
                destination_settler: self.config.destination_settler,
                fill_proof_verifier: self.config.fill_proof_verifier,
                exclusive_filler: self.config.exclusive_filler,
                exclusivity_ends: fill_deadline,
                fill_deadline,
                route_kind: DIRECT_ROUTE_KIND,
            },
            commitment: CompactCommitmentTerms {
                arbiter: self.config.origin_settler,
                sponsor: request.sponsor,
                nonce: request.compact_nonce,
                expires: request.compact_expires_unix,
                lock_tag: self.config.compact_lock_tag,
                token: request.quote.origin.input_token,
                amount: request.quote.amount_in,
                mandate,
            },
        })
    }

    #[tracing::instrument(skip_all)]
    pub fn authorize(
        &self,
        request: DirectOrderDraftRequest,
        sponsor_signature: Bytes,
        now_unix: u64,
    ) -> Result<DirectOrderAuthorization, SolventError> {
        let draft = self.draft(request.clone(), now_unix)?;
        let mandate = Mandate {
            orderId: draft.commitment.mandate.order_id,
            destinationChainId: U256::from(draft.commitment.mandate.destination_chain_id),
            destinationSettler: draft.commitment.mandate.destination_settler,
            fillProofVerifier: draft.commitment.mandate.fill_proof_verifier,
            outputToken: draft.commitment.mandate.output_token,
            minimumOutputAmount: draft.commitment.mandate.minimum_output_amount,
            recipient: draft.commitment.mandate.recipient,
            fillDeadline: U48::from(draft.commitment.mandate.fill_deadline),
            exclusiveFiller: draft.commitment.mandate.exclusive_filler,
            routeKind: draft.commitment.mandate.route_kind,
        };
        let compact = Compact {
            arbiter: draft.commitment.arbiter,
            sponsor: draft.commitment.sponsor,
            nonce: draft.commitment.nonce,
            expires: U256::from(draft.commitment.expires),
            lockTag: draft.commitment.lock_tag,
            token: draft.commitment.token,
            amount: draft.commitment.amount,
            mandate: mandate.clone(),
        };
        let domain = eip712_domain! {
            name: "The Compact",
            version: "1",
            chain_id: self.config.origin_chain.0,
            verifying_contract: self.config.compact,
        };
        let digest = compact.eip712_signing_hash(&domain);
        let signature = Signature::from_raw(&sponsor_signature)
            .map_err(|_| invalid("Compact sponsor signature is malformed"))?;
        let recovered = signature
            .recover_address_from_prehash(&digest)
            .map_err(|_| invalid("Compact sponsor signature is invalid"))?;
        if recovered != request.sponsor {
            return Err(invalid(
                "Compact sponsor signature belongs to another account",
            ));
        }

        Ok(DirectOrderAuthorization {
            quote: request.quote,
            order_id: draft.order_id,
            order: DirectSettlementOrder {
                user: draft.order.user,
                nonce: draft.order.nonce,
                origin_chain_id: draft.order.origin_chain_id,
                origin_settler: draft.order.origin_settler,
                compact: draft.order.compact,
                compact_id: draft.order.compact_id,
                compact_expires: draft.order.compact_expires,
                input_token: draft.order.input_token,
                input_amount: draft.order.input_amount,
                destination_chain_id: draft.order.destination_chain_id,
                output_token: draft.order.output_token,
                minimum_output_amount: draft.order.minimum_output_amount,
                recipient: draft.order.recipient,
                destination_settler: draft.order.destination_settler,
                fill_proof_verifier: draft.order.fill_proof_verifier,
                exclusive_filler: draft.order.exclusive_filler,
                exclusivity_ends: draft.order.exclusivity_ends,
                fill_deadline: draft.order.fill_deadline,
                route_kind: draft.order.route_kind,
            },
            mandate: DirectSettlementMandate {
                order_id: draft.commitment.mandate.order_id,
                destination_chain_id: draft.commitment.mandate.destination_chain_id,
                destination_settler: draft.commitment.mandate.destination_settler,
                fill_proof_verifier: draft.commitment.mandate.fill_proof_verifier,
                output_token: draft.commitment.mandate.output_token,
                minimum_output_amount: draft.commitment.mandate.minimum_output_amount,
                recipient: draft.commitment.mandate.recipient,
                fill_deadline: draft.commitment.mandate.fill_deadline,
                exclusive_filler: draft.commitment.mandate.exclusive_filler,
                route_kind: draft.commitment.mandate.route_kind,
            },
            compact_claim: CompactClaimAuthorization {
                sponsor_signature,
                sponsor: draft.commitment.sponsor,
                nonce: draft.commitment.nonce,
                expires: draft.commitment.expires,
                witness: mandate.eip712_hash_struct(),
                witness_typestring: MANDATE_WITNESS_TYPESTRING.to_string(),
                id: draft.order.compact_id,
                allocated_amount: draft.commitment.amount,
                claimant: draft.commitment.arbiter,
                claimant_amount: draft.commitment.amount,
            },
        })
    }
}

fn invalid(message: &str) -> SolventError {
    SolventError::InvalidCrossChain(message.to_string())
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct DirectOrderDraftRequest {
    pub quote: AggregateQuote,
    #[schema(value_type = String)]
    pub sponsor: Address,
    #[schema(value_type = String)]
    pub recipient: Address,
    #[schema(value_type = String)]
    pub order_nonce: U256,
    #[schema(value_type = String)]
    pub compact_nonce: U256,
    pub compact_expires_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct DirectOrderDraft {
    #[schema(value_type = String)]
    pub aggregate_id: AggregateQuoteId,
    #[schema(value_type = String)]
    pub order_id: CrossChainOrderId,
    #[schema(value_type = String)]
    pub compact: Address,
    pub order: SolventCompactOrder,
    pub commitment: CompactCommitmentTerms,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct SolventCompactOrder {
    #[schema(value_type = String)]
    pub user: Address,
    #[schema(value_type = String)]
    pub nonce: U256,
    pub origin_chain_id: u64,
    #[schema(value_type = String)]
    pub origin_settler: Address,
    #[schema(value_type = String)]
    pub compact: Address,
    #[schema(value_type = String)]
    pub compact_id: U256,
    pub compact_expires: u64,
    #[schema(value_type = String)]
    pub input_token: Address,
    #[schema(value_type = String)]
    pub input_amount: U256,
    pub destination_chain_id: u64,
    #[schema(value_type = String)]
    pub output_token: Address,
    #[schema(value_type = String)]
    pub minimum_output_amount: U256,
    #[schema(value_type = String)]
    pub recipient: Address,
    #[schema(value_type = String)]
    pub destination_settler: Address,
    #[schema(value_type = String)]
    pub fill_proof_verifier: Address,
    #[schema(value_type = String)]
    pub exclusive_filler: Address,
    pub exclusivity_ends: u64,
    pub fill_deadline: u64,
    pub route_kind: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct SolventCompactMandate {
    #[schema(value_type = String)]
    pub order_id: B256,
    pub destination_chain_id: u64,
    #[schema(value_type = String)]
    pub destination_settler: Address,
    #[schema(value_type = String)]
    pub fill_proof_verifier: Address,
    #[schema(value_type = String)]
    pub output_token: Address,
    #[schema(value_type = String)]
    pub minimum_output_amount: U256,
    #[schema(value_type = String)]
    pub recipient: Address,
    pub fill_deadline: u64,
    #[schema(value_type = String)]
    pub exclusive_filler: Address,
    pub route_kind: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct CompactCommitmentTerms {
    #[schema(value_type = String)]
    pub arbiter: Address,
    #[schema(value_type = String)]
    pub sponsor: Address,
    #[schema(value_type = String)]
    pub nonce: U256,
    pub expires: u64,
    #[schema(value_type = String)]
    pub lock_tag: FixedBytes<12>,
    #[schema(value_type = String)]
    pub token: Address,
    #[schema(value_type = String)]
    pub amount: U256,
    pub mandate: SolventCompactMandate,
}

fn compact_id(lock_tag: FixedBytes<12>, token: Address) -> U256 {
    let mut encoded = [0_u8; 32];
    encoded[..12].copy_from_slice(lock_tag.as_slice());
    encoded[12..].copy_from_slice(token.as_slice());
    U256::from_be_bytes(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solvent_core::crosschain::{aggregate_quote, leg_quote_id};
    use solvent_core::primitives::crosschain::{LegQuote, LegRole};
    use solvent_core::primitives::ledger::ReservationSource;
    use solvent_core::primitives::{MakerId, StrategyHash};

    const NOW: u64 = 1_800_000_000;

    fn address(byte: u8) -> Address {
        Address::repeat_byte(byte)
    }

    fn leg(role: LegRole) -> LegQuote {
        let (local_chain, remote_chain, input_token, output_token, amount_in, amount_out, sources) =
            match role {
                LegRole::Origin => (
                    ChainId(1),
                    ChainId(2),
                    address(0x11),
                    address(0x11),
                    U256::from(10_000),
                    U256::from(10_000),
                    Vec::new(),
                ),
                LegRole::Destination => (
                    ChainId(2),
                    ChainId(1),
                    address(0x12),
                    address(0x13),
                    U256::from(10_000),
                    U256::from(9_900),
                    vec![ReservationSource {
                        maker: MakerId(address(0x14)),
                        strategy_hash: StrategyHash(B256::repeat_byte(0x15)),
                        token: address(0x13),
                        amount: U256::from(9_900),
                    }],
                ),
            };
        let mut quote = LegQuote {
            quote_id: B256::ZERO,
            request_id: B256::repeat_byte(0x10),
            role,
            local_chain,
            remote_chain,
            input_token,
            output_token,
            amount_in,
            amount_out,
            route: CrossChainRoute::Direct,
            block_number: 50,
            expires_at_unix: NOW + 300,
            sources,
        };
        quote.quote_id = leg_quote_id(&quote);
        quote
    }

    fn builder() -> DirectOrderDraftBuilder {
        DirectOrderDraftBuilder::new(DirectOrderDraftConfig {
            origin_chain: ChainId(1),
            destination_chain: ChainId(2),
            origin_settler: address(0x21),
            compact: address(0x22),
            destination_settler: address(0x23),
            fill_proof_verifier: address(0x24),
            exclusive_filler: address(0x25),
            compact_lock_tag: FixedBytes::repeat_byte(0x26),
        })
        .expect("valid draft configuration")
    }

    fn request() -> DirectOrderDraftRequest {
        DirectOrderDraftRequest {
            quote: aggregate_quote(leg(LegRole::Origin), leg(LegRole::Destination), NOW)
                .expect("compatible quote"),
            sponsor: address(0x31),
            recipient: address(0x32),
            order_nonce: U256::from(7),
            compact_nonce: U256::from(8),
            compact_expires_unix: NOW + 400,
        }
    }

    #[test]
    fn draft_pins_the_wallet_commitment_to_the_aggregate_quote() {
        let draft = builder().draft(request(), NOW).expect("valid draft");

        assert_eq!(draft.aggregate_id, request().quote.id);
        assert_eq!(draft.order.input_amount, U256::from(10_000));
        assert_eq!(draft.order.minimum_output_amount, U256::from(9_900));
        assert_eq!(draft.order.fill_deadline, NOW + 300);
        assert_eq!(draft.commitment.arbiter, address(0x21));
        assert_eq!(draft.commitment.amount, U256::from(10_000));
        assert_eq!(draft.commitment.mandate.order_id, draft.order_id.0);
        assert_eq!(
            draft.order_id.0,
            alloy::primitives::b256!(
                "f88c808ee24f6ff747d373ae54d7a9b8490ca359ea92cf0ad79fed3003ea87bd"
            ),
        );

        let encoded_id = draft.order.compact_id.to_be_bytes::<32>();
        assert_eq!(&encoded_id[..12], &[0x26; 12]);
        assert_eq!(&encoded_id[12..], address(0x11).as_slice());
    }

    #[test]
    fn draft_rejects_an_execution_shape_the_direct_contract_cannot_fill() {
        let mut request = request();
        let mut destination = leg(LegRole::Destination);
        destination.sources.push(ReservationSource {
            maker: MakerId(address(0x41)),
            strategy_hash: StrategyHash(B256::repeat_byte(0x42)),
            token: address(0x13),
            amount: U256::from(1),
        });
        destination.quote_id = leg_quote_id(&destination);
        request.quote = aggregate_quote(leg(LegRole::Origin), destination, NOW)
            .expect("compatible multi-source quote");

        let error = builder()
            .draft(request, NOW)
            .expect_err("multi-source delivery must not be drafted");
        assert!(error.to_string().contains("exactly one"));
    }

    #[test]
    fn draft_requires_the_compact_to_outlive_destination_delivery() {
        let mut request = request();
        request.compact_expires_unix = request.quote.expires_at_unix;

        let error = builder()
            .draft(request, NOW)
            .expect_err("claim expiry must outlive fill");
        assert!(error.to_string().contains("later"));
    }

    #[test]
    fn draft_rejects_a_quote_without_enough_time_to_fill() {
        let error = builder()
            .draft(request(), NOW + 181)
            .expect_err("a nearly expired quote must not reach the wallet");

        assert!(error.to_string().contains("expires too soon"));
    }
}
