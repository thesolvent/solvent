//! Builds policy-authorized `Erc7683AquaFiller.fill(...)` calldata from a routed plan.

use std::sync::Arc;

use alloy::primitives::{keccak256, Address};
use alloy::sol_types::{SolCall, SolValue};
use async_trait::async_trait;

use solvent_core::deps::execution::ExecutionAuthorizer;
use solvent_core::deps::ingest::{FillBuilder, FillBuilderError, PreparedFill};
use solvent_core::primitives::ingest::{Intent, ProtocolId};
use solvent_core::primitives::registry::Snapshot;
use solvent_core::primitives::routing::RoutePlan;

use crate::ingest::aqua::AquaSourcePlanBuilder;

use super::codec::{fillCall, order_id, Solvent7683Order};

pub struct Erc7683FillBuilder {
    settler: Address,
    filler: Address,
    payment_recipient: Address,
    sources: AquaSourcePlanBuilder,
}

impl Erc7683FillBuilder {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app: Address,
        settler: Address,
        filler: Address,
        payment_recipient: Address,
        taker_credential: Address,
        authorizer: Arc<dyn ExecutionAuthorizer>,
    ) -> Self {
        Self {
            settler,
            filler,
            payment_recipient,
            sources: AquaSourcePlanBuilder::new(app, taker_credential, authorizer),
        }
    }
}

#[async_trait]
impl FillBuilder for Erc7683FillBuilder {
    #[tracing::instrument(skip_all, fields(intent = %intent.id))]
    async fn build(
        &self,
        intent: &Intent,
        plan: &RoutePlan,
        snapshot: &Snapshot,
    ) -> Result<PreparedFill, FillBuilderError> {
        if intent.protocol != ProtocolId::Erc7683 {
            return Err(FillBuilderError::InvalidOrder);
        }
        let order = Solvent7683Order::abi_decode(&intent.raw)
            .map_err(|_| FillBuilderError::InvalidOrder)?;
        if order.settler != self.settler {
            return Err(FillBuilderError::InvalidOrder);
        }
        let context_hash =
            keccak256((self.settler, order_id(&order), self.payment_recipient).abi_encode());
        let sources = self.sources.build(plan, context_hash, snapshot).await?;
        let source_data = sources.abi_encode().into();
        let calldata = fillCall {
            orderData: intent.raw.clone(),
            permitSignature: intent.signature.clone(),
            sourceData: source_data,
            paymentRecipient: self.payment_recipient,
        }
        .abi_encode()
        .into();
        Ok(PreparedFill::new(self.filler, calldata))
    }
}

#[cfg(test)]
mod tests {
    use alloy::primitives::{address, Bytes, B256, U256};
    use async_trait::async_trait;

    use solvent_core::deps::execution::{ExecutionAuthorizer, ExecutionAuthorizerError};
    use solvent_core::primitives::execution::{ExecutionAuthorization, PolicySignature};
    use solvent_core::primitives::ingest::{AmountCurve, IntentInput, IntentOutput};
    use solvent_core::primitives::registry::AquaEvent;
    use solvent_core::primitives::routing::RouteLeg;
    use solvent_core::primitives::{ChainId, IntentId, MakerId, StrategyHash};

    use super::*;
    use crate::execution::filler::Order;

    struct FakeAuthorizer;

    #[async_trait]
    impl ExecutionAuthorizer for FakeAuthorizer {
        async fn authorize(
            &self,
            _: &ExecutionAuthorization,
        ) -> Result<PolicySignature, ExecutionAuthorizerError> {
            Ok(PolicySignature::new(Bytes::from(vec![0xAA; 65])))
        }
    }

    #[tokio::test]
    async fn targets_erc_filler_and_binds_settler_order_and_recipient() {
        let app = address!("1111111111111111111111111111111111111111");
        let settler = address!("2222222222222222222222222222222222222222");
        let filler = address!("3333333333333333333333333333333333333333");
        let recipient = address!("4444444444444444444444444444444444444444");
        let credential = address!("5555555555555555555555555555555555555555");
        let maker = MakerId(address!("6666666666666666666666666666666666666666"));
        let strategy_hash = StrategyHash(B256::repeat_byte(0x77));
        let token_in = address!("8888888888888888888888888888888888888888");
        let token_out = address!("9999999999999999999999999999999999999999");
        let source_order = Order {
            maker: maker.0,
            traits: U256::from(3u8),
            data: Bytes::from([vec![0x0e, 20], credential.to_vec(), vec![0xAB]].concat()),
        };
        let mut snapshot = Snapshot::default();
        snapshot.apply(AquaEvent::Shipped {
            maker,
            app,
            strategy_hash,
            strategy: source_order.abi_encode().into(),
        });
        let order = Solvent7683Order {
            settler,
            user: recipient,
            chainId: U256::from(31337u64),
            inputToken: token_in,
            inputAmount: U256::from(1_005u64),
            outputToken: token_out,
            outputAmount: U256::from(500u64),
            recipient,
            executorFee: U256::from(5u8),
            nonce: U256::from(9u8),
            deadline: U256::from(2_000u64),
        };
        let intent = Intent::new(
            IntentId(order_id(&order)),
            ProtocolId::Erc7683,
            IntentInput::new(token_in, AmountCurve::scalar(order.inputAmount)),
            Some(U256::from(1_000u64)),
            vec![IntentOutput::new(
                token_out,
                AmountCurve::scalar(order.outputAmount),
                recipient,
            )],
            2_000,
            None,
            settler,
            ChainId(31337),
            order.abi_encode().into(),
            Bytes::from(vec![0xCC; 65]),
            1_000,
        );
        let plan = RoutePlan::new(
            intent.id,
            vec![RouteLeg {
                maker,
                strategy_hash,
                token_in,
                token_out,
                amount_in: U256::from(1_000u64),
                amount_out: U256::from(500u64),
            }],
            U256::ZERO,
            0.0,
        );
        let builder = Erc7683FillBuilder::new(
            app,
            settler,
            filler,
            recipient,
            credential,
            Arc::new(FakeAuthorizer),
        );
        let prepared = builder
            .build(&intent, &plan, &snapshot)
            .await
            .expect("prepared fill");
        let call = fillCall::abi_decode(&prepared.calldata).expect("decode call");
        let context = keccak256((settler, order_id(&order), recipient).abi_encode());
        let expected_sources = builder
            .sources
            .build(&plan, context, &snapshot)
            .await
            .expect("source plan");

        assert_eq!(prepared.target, filler);
        assert_eq!(call.paymentRecipient, recipient);
        assert_eq!(call.orderData, intent.raw);
        assert_eq!(call.permitSignature, intent.signature);
        assert_eq!(call.sourceData, Bytes::from(expected_sources.abi_encode()));
    }
}
