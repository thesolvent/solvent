use alloy::primitives::{keccak256, Address, B256, U256};
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolCall, SolStruct, SolValue};
use async_trait::async_trait;
use solvent_core::deps::crosschain::{StepValidator, StepValidatorError};
use solvent_core::primitives::crosschain::{
    ChainExecutionPlan, CrossChainRoute, PreparedStep, RemoteCommand, StepValidationContext,
};

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
        function fillCredit(
            SolventCrossChainOrder order,
            SolventMandate mandate,
            MakerCreditQuote quote,
            bytes makerSignature
        ) external;
        function completeRepayment(bytes32 orderId, bytes message, bytes attestation) external;
        function confirmDirectRepayment(bytes32 orderId, bytes repaymentProof) external;
    }

    interface ICompactOriginSettler {
        function settleDirect(
            SolventCrossChainOrder order,
            SolventMandate mandate,
            bytes fillProof,
            Claim compactClaim
        ) external;
        function settleRouted(
            SolventCrossChainOrder order,
            SolventMandate mandate,
            bytes fillProof,
            Claim compactClaim,
            SwapOrder makerOrder
        ) external;
    }

    interface ICcipProofOutbox {
        function dispatch(bytes32 orderId, bytes envelope) external payable returns (bytes32 messageId);
    }
}

/// Validates the exact settlement-contract ABI before an authenticated plan becomes durable.
pub struct AlloyStepValidator {
    operator: Address,
    wrapped_native: Address,
    origin_application: Option<Address>,
    destination_application: Option<Address>,
}

impl AlloyStepValidator {
    pub fn new(operator: Address, wrapped_native: Address) -> Self {
        Self {
            operator,
            wrapped_native,
            origin_application: None,
            destination_application: None,
        }
    }

    pub fn with_applications(
        mut self,
        origin_application: Address,
        destination_application: Address,
    ) -> Self {
        self.origin_application = Some(origin_application);
        self.destination_application = Some(destination_application);
        self
    }
}

fn validate_plan_proofs(
    validator: &AlloyStepValidator,
    context: &StepValidationContext,
    plan: &ChainExecutionPlan,
) -> Result<(), StepValidatorError> {
    let Some(deliver) = plan
        .steps
        .iter()
        .find(|step| step.command == RemoteCommand::Deliver)
    else {
        return Ok(());
    };
    let Some(dispatch) = plan
        .steps
        .iter()
        .find(|step| step.command == RemoteCommand::DispatchFillProof)
    else {
        return Ok(());
    };
    let dispatch = decode::<ICcipProofOutbox::dispatchCall>(&dispatch.calldata)?;
    let envelope = decode_envelope(&dispatch.envelope, 0)?;
    let fill = decode_fill(&envelope.payload)?;
    let destination_application =
        required_application(validator.destination_application, "destination application")?;

    match context.quote.origin.route {
        CrossChainRoute::Direct => {
            let call = decode::<ICrossChainAquaApp::fillDirectCall>(&deliver.calldata)?;
            validator.validate_applications(&call.order)?;
            let domain = eip712_domain! {
                name: "Solvent Cross-Chain Aqua App",
                version: "1",
                chain_id: context.quote.destination.local_chain.0,
                verifying_contract: destination_application,
            };
            if fill.recipient != call.order.recipient
                || fill.originStrategyHash != call.quote.originStrategyHash
                || fill.makerQuoteHash != call.quote.eip712_signing_hash(&domain)
            {
                return invalid("fill proof does not match the signed direct delivery terms");
            }
        }
        CrossChainRoute::Cctp => {
            let call = decode::<ICrossChainAquaApp::fillCreditCall>(&deliver.calldata)?;
            validator.validate_applications(&call.order)?;
            let domain = eip712_domain! {
                name: "Solvent Cross-Chain Aqua App",
                version: "1",
                chain_id: context.quote.destination.local_chain.0,
                verifying_contract: destination_application,
            };
            if fill.recipient != call.order.recipient
                || !fill.originStrategyHash.is_zero()
                || fill.makerQuoteHash != call.quote.eip712_signing_hash(&domain)
            {
                return invalid("fill proof does not match the signed credit delivery terms");
            }
        }
    }
    Ok(())
}

impl AlloyStepValidator {
    fn validate_applications(
        &self,
        order: &SolventCrossChainOrder,
    ) -> Result<(), StepValidatorError> {
        let origin = required_application(self.origin_application, "origin application")?;
        let destination =
            required_application(self.destination_application, "destination application")?;
        if order.originSettler != origin || order.destinationSettler != destination {
            return invalid("settlement applications do not match this service configuration");
        }
        Ok(())
    }
}

#[async_trait]
impl StepValidator for AlloyStepValidator {
    async fn validate(
        &self,
        context: &StepValidationContext,
        step: &PreparedStep,
    ) -> Result<(), StepValidatorError> {
        if !step.value.is_zero() {
            return invalid(
                "staged native value must be zero; submission materializes bridge fees",
            );
        }
        match (step.command, context.quote.origin.route) {
            (RemoteCommand::Deliver, CrossChainRoute::Direct) => {
                let call = decode::<ICrossChainAquaApp::fillDirectCall>(&step.calldata)?;
                validate_order(context, step, &call.order, self.operator)?;
                validate_mandate(context, &call.order, &call.mandate)?;
                if call.quote.orderId != context.order_id.0
                    || call.quote.outputAmount != context.quote.amount_out
                    || call.quote.repaymentAmount != context.quote.amount_in
                {
                    return invalid("direct maker quote does not match aggregate terms");
                }
                validate_destination_source(
                    context,
                    call.quote.maker,
                    call.quote.destinationStrategyHash,
                    call.quote.outputAmount,
                    self.wrapped_native,
                )?;
            }
            (RemoteCommand::Deliver, CrossChainRoute::Cctp) => {
                let call = decode::<ICrossChainAquaApp::fillCreditCall>(&step.calldata)?;
                validate_order(context, step, &call.order, self.operator)?;
                validate_mandate(context, &call.order, &call.mandate)?;
                if call.quote.orderId != context.order_id.0
                    || call.quote.outputAmount != context.quote.amount_out
                    || call.quote.usdcDue != context.quote.destination.amount_in
                    || call.quote.maxCctpFee != context.quote.bridge_fee
                {
                    return invalid("credit maker quote does not match aggregate terms");
                }
                validate_destination_source(
                    context,
                    call.quote.maker,
                    call.quote.destinationStrategyHash,
                    call.quote.outputAmount,
                    self.wrapped_native,
                )?;
            }
            (RemoteCommand::ClaimOrigin, CrossChainRoute::Direct) => {
                let call = decode::<ICompactOriginSettler::settleDirectCall>(&step.calldata)?;
                self.validate_applications(&call.order)?;
                validate_order(context, step, &call.order, self.operator)?;
                validate_mandate(context, &call.order, &call.mandate)?;
                validate_proof_id(
                    &call.fillProof,
                    proof_id(
                        context.quote.destination.local_chain.0,
                        call.order.destinationSettler,
                        context.order_id.0,
                        0,
                    ),
                    "fill proof",
                )?;
            }
            (RemoteCommand::ClaimOrigin, CrossChainRoute::Cctp) => {
                let call = decode::<ICompactOriginSettler::settleRoutedCall>(&step.calldata)?;
                self.validate_applications(&call.order)?;
                validate_order(context, step, &call.order, self.operator)?;
                validate_mandate(context, &call.order, &call.mandate)?;
                validate_origin_swap_source(context, &call.makerOrder)?;
                validate_proof_id(
                    &call.fillProof,
                    proof_id(
                        context.quote.destination.local_chain.0,
                        call.order.destinationSettler,
                        context.order_id.0,
                        0,
                    ),
                    "fill proof",
                )?;
            }
            (RemoteCommand::DispatchFillProof, _) => {
                let call = decode::<ICcipProofOutbox::dispatchCall>(&step.calldata)?;
                if call.orderId != context.order_id.0 {
                    return invalid("proof dispatch is bound to another order");
                }
                validate_fill_envelope(context, &call.envelope, self.destination_application)?;
            }
            (RemoteCommand::DispatchRepayment, CrossChainRoute::Direct) => {
                let call = decode::<ICcipProofOutbox::dispatchCall>(&step.calldata)?;
                if call.orderId != context.order_id.0 {
                    return invalid("proof dispatch is bound to another order");
                }
                validate_repayment_envelope(context, &call.envelope, self.origin_application)?;
            }
            (RemoteCommand::DispatchRepayment, CrossChainRoute::Cctp) => {
                return invalid("CCTP routes do not dispatch direct repayment proofs");
            }
            (RemoteCommand::CloseDestination, CrossChainRoute::Direct) => {
                let call =
                    decode::<ICrossChainAquaApp::confirmDirectRepaymentCall>(&step.calldata)?;
                if call.orderId != context.order_id.0 {
                    return invalid("direct repayment close is bound to another order");
                }
                let origin_application =
                    required_application(self.origin_application, "origin application")?;
                validate_proof_id(
                    &call.repaymentProof,
                    proof_id(
                        context.quote.origin.local_chain.0,
                        origin_application,
                        context.order_id.0,
                        1,
                    ),
                    "repayment proof",
                )?;
            }
            (RemoteCommand::CloseDestination, CrossChainRoute::Cctp) => {
                let call = decode::<ICrossChainAquaApp::completeRepaymentCall>(&step.calldata)?;
                if call.orderId != context.order_id.0 {
                    return invalid("CCTP completion is bound to another order");
                }
            }
        }
        Ok(())
    }

    async fn validate_plan(
        &self,
        context: &StepValidationContext,
        plan: &ChainExecutionPlan,
    ) -> Result<(), StepValidatorError> {
        for step in &plan.steps {
            self.validate(context, step).await?;
        }
        validate_plan_proofs(self, context, plan)
    }
}

fn validate_destination_source(
    context: &StepValidationContext,
    maker: Address,
    strategy_hash: B256,
    amount: U256,
    wrapped_native: Address,
) -> Result<(), StepValidatorError> {
    let [source] = context.quote.destination.sources.as_slice() else {
        return invalid("destination settlement requires exactly one quoted maker source");
    };
    let reserve_token = if context.quote.destination.output_token.is_zero() {
        if wrapped_native.is_zero() {
            return invalid("native delivery requires a configured wrapped-native token");
        }
        wrapped_native
    } else {
        context.quote.destination.output_token
    };
    if source.maker.0 != maker
        || source.strategy_hash.0 != strategy_hash
        || source.token != reserve_token
        || source.amount != amount
    {
        return invalid("destination maker source does not match the locally issued quote");
    }
    Ok(())
}

fn validate_origin_swap_source(
    context: &StepValidationContext,
    maker_order: &SwapOrder,
) -> Result<(), StepValidatorError> {
    let [source] = context.quote.origin.sources.as_slice() else {
        return invalid("routed origin settlement requires exactly one quoted maker source");
    };
    if source.maker.0 != maker_order.maker
        || source.strategy_hash.0 != keccak256(maker_order.abi_encode())
        || source.token != context.quote.origin.output_token
        || source.amount != context.quote.origin.amount_out
    {
        return invalid("origin swap source does not match the locally issued quote");
    }
    Ok(())
}

fn validate_fill_envelope(
    context: &StepValidationContext,
    encoded: &[u8],
    destination_application: Option<Address>,
) -> Result<(), StepValidatorError> {
    let envelope = decode_envelope(encoded, 0)?;
    let fill = decode_fill(&envelope.payload)?;
    let application = required_application(destination_application, "destination application")?;
    let [source] = context.quote.destination.sources.as_slice() else {
        return invalid("fill proof requires exactly one destination reservation source");
    };
    let (route_kind, repayment_token, repayment_amount, max_cctp_fee) =
        match context.quote.origin.route {
            CrossChainRoute::Direct => (
                1,
                context.quote.origin.input_token,
                context.quote.amount_in,
                U256::ZERO,
            ),
            CrossChainRoute::Cctp => (
                0,
                context.quote.destination.input_token,
                context.quote.destination.amount_in,
                context.quote.bridge_fee,
            ),
        };
    let expected_id = proof_id(
        context.quote.destination.local_chain.0,
        application,
        context.order_id.0,
        0,
    );
    if fill.orderId != context.order_id.0
        || fill.routeKind != route_kind
        || fill.destinationChainId != U256::from(context.quote.destination.local_chain.0)
        || fill.destinationApp != application
        || fill.outputToken != context.quote.destination.output_token
        || fill.outputAmount != context.quote.amount_out
        || fill.destinationMaker != source.maker.0
        || fill.destinationStrategyHash != source.strategy_hash.0
        || fill.repaymentToken != repayment_token
        || fill.repaymentAmount != repayment_amount
        || fill.maxCctpFee != max_cctp_fee
        || fill.makerQuoteHash.is_zero()
        || fill.fillId != expected_id
    {
        return invalid("fill proof does not match the admitted order and quote");
    }
    Ok(())
}

fn validate_repayment_envelope(
    context: &StepValidationContext,
    encoded: &[u8],
    origin_application: Option<Address>,
) -> Result<(), StepValidatorError> {
    let envelope = decode_envelope(encoded, 1)?;
    let repayment = decode_repayment(&envelope.payload)?;
    let application = required_application(origin_application, "origin application")?;
    let [source] = context.quote.destination.sources.as_slice() else {
        return invalid("repayment proof requires exactly one destination reservation source");
    };
    let expected_id = proof_id(
        context.quote.origin.local_chain.0,
        application,
        context.order_id.0,
        1,
    );
    if repayment.orderId != context.order_id.0
        || repayment.originChainId != U256::from(context.quote.origin.local_chain.0)
        || repayment.originSettler != application
        || repayment.maker != source.maker.0
        || repayment.repaymentToken != context.quote.origin.input_token
        || repayment.repaymentAmount != context.quote.amount_in
        || repayment.repaymentId != expected_id
    {
        return invalid("repayment proof does not match the admitted order and quote");
    }
    Ok(())
}

fn decode_envelope(encoded: &[u8], kind: u8) -> Result<ProofEnvelope, StepValidatorError> {
    let envelope = ProofEnvelope::abi_decode(encoded)
        .map_err(|_| StepValidatorError::Invalid("proof envelope ABI is invalid".to_string()))?;
    if envelope.abi_encode() != encoded || envelope.version != 1 || envelope.kind != kind {
        return invalid("proof envelope is not canonical for its command");
    }
    Ok(envelope)
}

fn decode_fill(encoded: &[u8]) -> Result<VerifiedFill, StepValidatorError> {
    let fill = VerifiedFill::abi_decode(encoded)
        .map_err(|_| StepValidatorError::Invalid("verified fill ABI is invalid".to_string()))?;
    if fill.abi_encode() != encoded {
        return invalid("verified fill payload is not canonical ABI");
    }
    Ok(fill)
}

fn decode_repayment(encoded: &[u8]) -> Result<VerifiedRepayment, StepValidatorError> {
    let repayment = VerifiedRepayment::abi_decode(encoded).map_err(|_| {
        StepValidatorError::Invalid("verified repayment ABI is invalid".to_string())
    })?;
    if repayment.abi_encode() != encoded {
        return invalid("verified repayment payload is not canonical ABI");
    }
    Ok(repayment)
}

fn proof_id(chain_id: u64, application: Address, order_id: B256, kind: u8) -> B256 {
    keccak256(
        ProofIdMaterial {
            chainId: U256::from(chain_id),
            application,
            orderId: order_id,
            kind,
        }
        .abi_encode(),
    )
}

fn validate_proof_id(
    encoded: &[u8],
    expected: B256,
    label: &str,
) -> Result<(), StepValidatorError> {
    let actual = validate_encoded_word(encoded, label)?;
    if actual != expected {
        return invalid(&format!("{label} ID does not match the admitted order"));
    }
    Ok(())
}

fn validate_encoded_word(encoded: &[u8], label: &str) -> Result<B256, StepValidatorError> {
    if encoded.len() != 32 {
        return invalid(&format!("{label} must be exactly one ABI bytes32 word"));
    }
    B256::abi_decode(encoded)
        .map_err(|_| StepValidatorError::Invalid(format!("{label} ABI is invalid")))
}

fn required_application(
    application: Option<Address>,
    label: &str,
) -> Result<Address, StepValidatorError> {
    match application {
        Some(application) if !application.is_zero() => Ok(application),
        _ => invalid(&format!("{label} is not configured for proof validation")),
    }
}

fn decode<Call: SolCall>(calldata: &[u8]) -> Result<Call, StepValidatorError> {
    Call::abi_decode(calldata)
        .map_err(|_| StepValidatorError::Invalid("calldata selector or ABI is invalid".to_string()))
}

fn validate_order(
    context: &StepValidationContext,
    step: &PreparedStep,
    order: &SolventCrossChainOrder,
    operator: Address,
) -> Result<(), StepValidatorError> {
    let route_kind = match context.quote.origin.route {
        CrossChainRoute::Direct => 1,
        CrossChainRoute::Cctp => 0,
    };
    if B256::from(order.eip712_hash_struct()) != context.order_id.0
        || order.originChainId != U256::from(context.quote.origin.local_chain.0)
        || order.destinationChainId != U256::from(context.quote.destination.local_chain.0)
        || order.inputToken != context.quote.origin.input_token
        || order.inputAmount != context.quote.amount_in
        || order.outputToken != context.quote.destination.output_token
        || order.minimumOutputAmount != context.quote.amount_out
        || order.routeKind != route_kind
    {
        return invalid("settlement order does not match aggregate terms");
    }
    match step.command {
        RemoteCommand::Deliver
            if order.destinationSettler != step.target || order.exclusiveFiller != operator =>
        {
            invalid("destination settlement authority does not match this service")
        }
        RemoteCommand::ClaimOrigin if order.originSettler != step.target => {
            invalid("origin settlement authority does not match this service")
        }
        _ => Ok(()),
    }
}

fn validate_mandate(
    context: &StepValidationContext,
    order: &SolventCrossChainOrder,
    mandate: &SolventMandate,
) -> Result<(), StepValidatorError> {
    if mandate.orderId != context.order_id.0
        || mandate.destinationChainId != order.destinationChainId
        || mandate.destinationSettler != order.destinationSettler
        || mandate.fillProofVerifier != order.fillProofVerifier
        || mandate.outputToken != order.outputToken
        || mandate.minimumOutputAmount != order.minimumOutputAmount
        || mandate.recipient != order.recipient
        || mandate.fillDeadline != order.fillDeadline
        || mandate.exclusiveFiller != order.exclusiveFiller
        || mandate.routeKind != order.routeKind
    {
        return invalid("mandate does not match the settlement order");
    }
    Ok(())
}

fn invalid<T>(message: &str) -> Result<T, StepValidatorError> {
    Err(StepValidatorError::Invalid(message.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Bytes, Uint};
    use solvent_core::primitives::crosschain::{
        AggregateQuote, LegQuote, LegRole, PreparedStep, StepValidationContext,
    };
    use solvent_core::primitives::ledger::ReservationSource;
    use solvent_core::primitives::{
        AggregateQuoteId, ChainId, CrossChainOrderId, MakerId, StrategyHash,
    };

    type U48 = Uint<48, 1>;

    struct ValidDirect {
        context: StepValidationContext,
        step: PreparedStep,
    }

    fn valid_direct() -> ValidDirect {
        let operator = Address::repeat_byte(0x10);
        let destination = Address::repeat_byte(0x11);
        let order = SolventCrossChainOrder {
            user: Address::repeat_byte(0x12),
            nonce: U256::from(1),
            originChainId: U256::from(1),
            originSettler: Address::repeat_byte(0x13),
            compact: Address::repeat_byte(0x14),
            compactId: U256::from(2),
            compactExpires: U256::from(2_000),
            inputToken: Address::repeat_byte(0x15),
            inputAmount: U256::from(1_000),
            destinationChainId: U256::from(42_161),
            outputToken: Address::repeat_byte(0x16),
            minimumOutputAmount: U256::from(850),
            recipient: Address::repeat_byte(0x17),
            destinationSettler: destination,
            fillProofVerifier: Address::repeat_byte(0x18),
            exclusiveFiller: operator,
            exclusivityEnds: U48::from(1_500),
            fillDeadline: U48::from(1_900),
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
            maker: Address::repeat_byte(0x19),
            destinationStrategyHash: B256::repeat_byte(0x20),
            originStrategyHash: B256::repeat_byte(0x21),
            outputAmount: U256::from(850),
            repaymentAmount: U256::from(1_000),
            nonce: U256::from(3),
            expires: U48::from(1_800),
        };
        let calldata = ICrossChainAquaApp::fillDirectCall {
            order,
            mandate,
            quote: maker_quote,
            makerSignature: Bytes::from_static(b"signature"),
        }
        .abi_encode()
        .into();
        let origin = LegQuote {
            quote_id: B256::repeat_byte(0x30),
            request_id: B256::repeat_byte(0x31),
            role: LegRole::Origin,
            local_chain: ChainId(1),
            remote_chain: ChainId(42_161),
            input_token: Address::repeat_byte(0x15),
            output_token: Address::repeat_byte(0x32),
            amount_in: U256::from(1_000),
            amount_out: U256::from(1_000),
            route: CrossChainRoute::Direct,
            price_impact_bps: None,
            block_number: 10,
            expires_at_unix: 2_000,
            sources: Vec::new(),
        };
        let destination_quote = LegQuote {
            quote_id: B256::repeat_byte(0x33),
            request_id: origin.request_id,
            role: LegRole::Destination,
            local_chain: ChainId(42_161),
            remote_chain: ChainId(1),
            input_token: Address::repeat_byte(0x34),
            output_token: Address::repeat_byte(0x16),
            amount_in: U256::from(900),
            amount_out: U256::from(850),
            route: CrossChainRoute::Direct,
            price_impact_bps: None,
            block_number: 11,
            expires_at_unix: 2_000,
            sources: vec![ReservationSource {
                maker: MakerId(Address::repeat_byte(0x19)),
                strategy_hash: StrategyHash(B256::repeat_byte(0x20)),
                token: Address::repeat_byte(0x16),
                amount: U256::from(850),
            }],
        };
        let context = StepValidationContext {
            order_id,
            quote: AggregateQuote {
                id: AggregateQuoteId(B256::repeat_byte(0x35)),
                origin,
                destination: destination_quote,
                amount_in: U256::from(1_000),
                amount_out: U256::from(850),
                bridge_fee: U256::ZERO,
                price_impact_bps: None,
                cctp_finality_threshold: None,
                expires_at_unix: 2_000,
            },
            role: LegRole::Destination,
        };
        ValidDirect {
            context,
            step: PreparedStep {
                command: RemoteCommand::Deliver,
                target: destination,
                value: U256::ZERO,
                calldata,
            },
        }
    }

    #[tokio::test]
    async fn direct_delivery_binds_order_mandate_authority_and_quote_terms() {
        let case = valid_direct();
        AlloyStepValidator::new(Address::repeat_byte(0x10), Address::repeat_byte(0x90))
            .validate(&case.context, &case.step)
            .await
            .expect("valid direct delivery");
    }

    #[tokio::test]
    async fn delivery_rejects_quoted_output_substitution() {
        let mut case = valid_direct();
        case.context.quote.amount_out = U256::from(849);
        assert!(
            AlloyStepValidator::new(Address::repeat_byte(0x10), Address::repeat_byte(0x90))
                .validate(&case.context, &case.step)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn delivery_rejects_a_different_reserved_maker_source() {
        let mut case = valid_direct();
        case.context.quote.destination.sources[0].maker = MakerId(Address::repeat_byte(0x99));
        assert!(
            AlloyStepValidator::new(Address::repeat_byte(0x10), Address::repeat_byte(0x90))
                .validate(&case.context, &case.step)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn delivery_rejects_a_multi_source_quote_the_contract_cannot_execute() {
        let mut case = valid_direct();
        let second = case.context.quote.destination.sources[0].clone();
        case.context.quote.destination.sources.push(second);
        assert!(
            AlloyStepValidator::new(Address::repeat_byte(0x10), Address::repeat_byte(0x90))
                .validate(&case.context, &case.step)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn command_rejects_another_contract_selector() {
        let mut case = valid_direct();
        case.step.command = RemoteCommand::CloseDestination;
        assert!(
            AlloyStepValidator::new(Address::repeat_byte(0x10), Address::repeat_byte(0x90))
                .validate(&case.context, &case.step)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn stage_rejects_client_supplied_native_value() {
        let mut case = valid_direct();
        case.step.value = U256::from(1);
        assert!(
            AlloyStepValidator::new(Address::repeat_byte(0x10), Address::repeat_byte(0x90))
                .validate(&case.context, &case.step)
                .await
                .is_err()
        );
    }

    #[test]
    fn routed_origin_binds_the_swapvm_order_to_its_reserved_source() {
        let mut case = valid_direct();
        let maker_order = SwapOrder {
            maker: Address::repeat_byte(0x81),
            traits: U256::from(7),
            data: Bytes::from_static(b"maker program"),
        };
        case.context.quote.origin.output_token = Address::repeat_byte(0x82);
        case.context.quote.origin.amount_out = U256::from(901);
        case.context.quote.origin.sources = vec![ReservationSource {
            maker: MakerId(maker_order.maker),
            strategy_hash: StrategyHash(keccak256(maker_order.abi_encode())),
            token: case.context.quote.origin.output_token,
            amount: case.context.quote.origin.amount_out,
        }];
        validate_origin_swap_source(&case.context, &maker_order).expect("bound maker order");

        case.context.quote.origin.sources[0].strategy_hash = StrategyHash(B256::repeat_byte(0x83));
        assert!(validate_origin_swap_source(&case.context, &maker_order).is_err());
    }

    #[tokio::test]
    async fn native_delivery_binds_the_configured_wrapped_token_source() {
        let mut case = valid_direct();
        let mut call = ICrossChainAquaApp::fillDirectCall::abi_decode(&case.step.calldata)
            .expect("decode direct call");
        call.order.outputToken = Address::ZERO;
        let order_id = CrossChainOrderId(call.order.eip712_hash_struct());
        call.mandate.orderId = order_id.0;
        call.mandate.outputToken = Address::ZERO;
        call.quote.orderId = order_id.0;
        case.context.order_id = order_id;
        case.context.quote.destination.output_token = Address::ZERO;
        case.context.quote.destination.sources[0].token = Address::repeat_byte(0x90);
        case.step.calldata = call.abi_encode().into();

        AlloyStepValidator::new(Address::repeat_byte(0x10), Address::repeat_byte(0x90))
            .validate(&case.context, &case.step)
            .await
            .expect("native delivery backed by configured wrapped token");

        case.context.quote.destination.sources[0].token = Address::repeat_byte(0x91);
        assert!(
            AlloyStepValidator::new(Address::repeat_byte(0x10), Address::repeat_byte(0x90))
                .validate(&case.context, &case.step)
                .await
                .is_err()
        );
    }
}
