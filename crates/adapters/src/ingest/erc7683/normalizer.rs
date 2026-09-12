//! Validates a same-chain Solvent ERC-7683 order and recovers its Permit2 signer.

use alloy::primitives::{Address, Signature, U256};
use alloy::sol_types::SolValue;

use solvent_core::deps::ingest::{NormalizeError, Normalizer};
use solvent_core::primitives::ingest::{
    AmountCurve, ExecutionFeePolicy, Intent, IntentInput, IntentOutput, IntentParts, ProtocolId,
    RawOrder,
};
use solvent_core::primitives::{ChainId, IntentId};

use super::codec::{order_id, permit_digest, Solvent7683Order};

#[non_exhaustive]
pub struct NormalizedErc7683 {
    pub intent: Intent,
    pub user: Address,
}

pub struct Erc7683Normalizer {
    chain: ChainId,
    permit2: Address,
    settler: Address,
    fee_policy: ExecutionFeePolicy,
}

impl Erc7683Normalizer {
    pub fn new(
        chain: ChainId,
        permit2: Address,
        settler: Address,
        fee_policy: ExecutionFeePolicy,
    ) -> Self {
        Self {
            chain,
            permit2,
            settler,
            fee_policy,
        }
    }

    pub fn normalize_order(&self, raw: &RawOrder) -> Result<NormalizedErc7683, NormalizeError> {
        let bad = || NormalizeError::Decode(ProtocolId::Erc7683);
        if raw.protocol != ProtocolId::Erc7683 || raw.chain != self.chain {
            return Err(bad());
        }
        let order = Solvent7683Order::abi_decode(&raw.payload).map_err(|_| bad())?;
        self.validate(&order, raw.observed_at)?;

        if !matches!(raw.signature.last(), Some(0 | 1 | 27 | 28)) {
            return Err(NormalizeError::Signature(ProtocolId::Erc7683));
        }
        let signature = Signature::from_raw(&raw.signature)
            .map_err(|_| NormalizeError::Signature(ProtocolId::Erc7683))?;
        let user = signature
            .recover_address_from_prehash(&permit_digest(&order, self.permit2, self.chain.0))
            .map_err(|_| NormalizeError::Signature(ProtocolId::Erc7683))?;
        if user != order.user {
            return Err(NormalizeError::Signature(ProtocolId::Erc7683));
        }

        let routing_input = order
            .inputAmount
            .checked_sub(order.executorFee)
            .filter(|amount| !amount.is_zero())
            .ok_or_else(bad)?;
        let intent = Intent::new(IntentParts {
            routing_input_limit: Some(routing_input),
            deadline: u64::try_from(order.deadline).map_err(|_| bad())?,
            settler: order.settler,
            raw: raw.payload.clone(),
            signature: raw.signature.clone(),
            observed_at: raw.observed_at,
            source: raw.source,
            ..IntentParts::new(
                IntentId(order_id(&order)),
                ProtocolId::Erc7683,
                order.user,
                IntentInput::new(order.inputToken, AmountCurve::scalar(order.inputAmount)),
                vec![IntentOutput::new(
                    order.outputToken,
                    AmountCurve::scalar(order.outputAmount),
                    order.recipient,
                )],
                raw.chain,
            )
        });
        Ok(NormalizedErc7683 { intent, user })
    }

    fn validate(&self, order: &Solvent7683Order, observed_at: u64) -> Result<(), NormalizeError> {
        let invalid = || NormalizeError::Invalid(ProtocolId::Erc7683);
        let addresses_valid = order.settler == self.settler
            && order.chainId == U256::from(self.chain.0)
            && order.user != Address::ZERO
            && order.inputToken != Address::ZERO
            && order.outputToken != Address::ZERO
            && order.recipient != Address::ZERO
            && order.inputToken != order.outputToken;
        let deadline_valid = order.deadline >= U256::from(observed_at);
        let expected_fee = self
            .fee_policy
            .fee(order.inputAmount)
            .map_err(|_| invalid())?;
        let amounts_valid = !order.inputAmount.is_zero()
            && !order.outputAmount.is_zero()
            && order.executorFee == expected_fee
            && order.executorFee < order.inputAmount;
        if addresses_valid && deadline_valid && amounts_valid {
            Ok(())
        } else {
            Err(invalid())
        }
    }
}

impl Normalizer for Erc7683Normalizer {
    fn normalize(&self, raw: &RawOrder) -> Result<Intent, NormalizeError> {
        self.normalize_order(raw)
            .map(|normalized| normalized.intent)
    }
}

#[cfg(test)]
mod tests {
    use alloy::primitives::{address, Bytes, B256};
    use alloy::signers::{local::PrivateKeySigner, SignerSync};

    use solvent_core::primitives::ingest::OrderSource;

    use super::*;
    use crate::ingest::erc7683::codec::permit_digest;

    const CHAIN_ID: u64 = 31337;

    fn signer(byte: u8) -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::repeat_byte(byte)).expect("valid test key")
    }

    fn policy() -> ExecutionFeePolicy {
        ExecutionFeePolicy::new(5).expect("valid fee policy")
    }

    fn order(user: Address) -> Solvent7683Order {
        Solvent7683Order {
            settler: address!("1111111111111111111111111111111111111111"),
            user,
            chainId: U256::from(CHAIN_ID),
            inputToken: address!("2222222222222222222222222222222222222222"),
            inputAmount: U256::from(10_001u64),
            outputToken: address!("3333333333333333333333333333333333333333"),
            outputAmount: U256::from(500u64),
            recipient: user,
            executorFee: U256::from(6u8),
            nonce: U256::from(7u8),
            deadline: U256::from(2_000u64),
        }
    }

    fn raw(order: &Solvent7683Order, signer: &PrivateKeySigner) -> RawOrder {
        let permit2 = address!("000000000022d473030f116ddee9f6b43ac78ba3");
        let signature = signer
            .sign_hash_sync(&permit_digest(order, permit2, CHAIN_ID))
            .expect("sign test order");
        RawOrder::new(
            ProtocolId::Erc7683,
            ChainId(CHAIN_ID),
            Bytes::from(order.abi_encode()),
            Bytes::from(signature.as_bytes()),
            1_000,
            OrderSource::Solvent,
        )
    }

    fn normalizer() -> Erc7683Normalizer {
        Erc7683Normalizer::new(
            ChainId(CHAIN_ID),
            address!("000000000022d473030f116ddee9f6b43ac78ba3"),
            address!("1111111111111111111111111111111111111111"),
            policy(),
        )
    }

    #[test]
    fn authenticates_and_keeps_gross_input_separate_from_routing() {
        let signer = signer(0x11);
        let order = order(signer.address());
        let normalized = normalizer()
            .normalize_order(&raw(&order, &signer))
            .expect("valid order");

        assert_eq!(normalized.user, signer.address());
        assert_eq!(normalized.intent.id.0, order_id(&order));
        assert_eq!(
            normalized.intent.input.curve.amount_at(1_000),
            order.inputAmount
        );
        assert_eq!(
            normalized.intent.routing_input_limit,
            Some(order.inputAmount - order.executorFee)
        );
    }

    #[test]
    fn rejects_fee_or_signature_that_does_not_match_the_signed_order() {
        let user_signer = signer(0x11);
        let mut changed = order(user_signer.address());
        let signed = raw(&changed, &user_signer);
        changed.executorFee += U256::from(1u8);
        let changed_raw = RawOrder::new(
            ProtocolId::Erc7683,
            ChainId(CHAIN_ID),
            Bytes::from(changed.abi_encode()),
            signed.signature.clone(),
            1_000,
            OrderSource::Solvent,
        );
        assert!(matches!(
            normalizer().normalize_order(&changed_raw),
            Err(NormalizeError::Invalid(ProtocolId::Erc7683))
        ));

        let other = signer(0x22);
        assert!(matches!(
            normalizer().normalize_order(&raw(&order(user_signer.address()), &other)),
            Err(NormalizeError::Signature(ProtocolId::Erc7683))
        ));

        let mut noncanonical = raw(&order(user_signer.address()), &user_signer);
        let mut bytes = noncanonical.signature.to_vec();
        let last = bytes
            .last_mut()
            .expect("test signature has a recovery byte");
        *last = 2;
        noncanonical.signature = bytes.into();
        assert!(matches!(
            normalizer().normalize_order(&noncanonical),
            Err(NormalizeError::Signature(ProtocolId::Erc7683))
        ));
    }

    #[test]
    fn rejects_every_settler_invariant_before_routing() {
        let user_signer = signer(0x11);
        let base = order(user_signer.address());
        let mut cases = Vec::new();

        let mut wrong_settler = base.clone();
        wrong_settler.settler = address!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        cases.push(wrong_settler);
        let mut wrong_chain = base.clone();
        wrong_chain.chainId += U256::from(1u8);
        cases.push(wrong_chain);
        let mut zero_token = base.clone();
        zero_token.inputToken = Address::ZERO;
        cases.push(zero_token);
        let mut same_token = base.clone();
        same_token.outputToken = same_token.inputToken;
        cases.push(same_token);
        let mut zero_output = base.clone();
        zero_output.outputAmount = U256::ZERO;
        cases.push(zero_output);
        let mut expired = base.clone();
        expired.deadline = U256::from(999u64);
        cases.push(expired);
        let mut wrong_fee = base;
        wrong_fee.executorFee += U256::from(1u8);
        cases.push(wrong_fee);

        for invalid in cases {
            assert!(matches!(
                normalizer().normalize_order(&raw(&invalid, &user_signer)),
                Err(NormalizeError::Invalid(ProtocolId::Erc7683))
            ));
        }
    }
}
