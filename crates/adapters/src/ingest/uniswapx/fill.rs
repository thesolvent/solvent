//! Builds policy-authorized `UniswapXAquaFiller.fill(...)` calldata from a routed plan.

use std::sync::Arc;

use alloy::primitives::{keccak256, Address};
use alloy::sol_types::{SolCall, SolValue};
use async_trait::async_trait;

use solvent_core::deps::execution::ExecutionAuthorizer;
use solvent_core::deps::ingest::{FillBuilder, FillBuilderError, PreparedFill};
use solvent_core::primitives::ingest::Intent;
use solvent_core::primitives::registry::Snapshot;
use solvent_core::primitives::routing::RoutePlan;

use crate::execution::filler::{fillCall, SignedOrder};
use crate::ingest::aqua::AquaSourcePlanBuilder;

/// The router is the deployment's Aqua app and hashes each source strategy. The authorizer binds
/// the exact routed amounts and signed UniswapX order before the filler can execute them.
pub struct UniswapXFillBuilder {
    filler: Address,
    sources: AquaSourcePlanBuilder,
}

impl UniswapXFillBuilder {
    pub fn new(
        app: Address,
        filler: Address,
        taker_credential: Address,
        authorizer: Arc<dyn ExecutionAuthorizer>,
    ) -> Self {
        Self {
            filler,
            sources: AquaSourcePlanBuilder::new(app, taker_credential, authorizer),
        }
    }

    #[cfg(test)]
    async fn source_for(
        &self,
        leg: &solvent_core::primitives::routing::RouteLeg,
        index: usize,
        context_hash: alloy::primitives::B256,
        snapshot: &Snapshot,
    ) -> Result<crate::execution::filler::SourceSwap, FillBuilderError> {
        self.sources
            .source_for(leg, index, context_hash, snapshot)
            .await
    }
}

#[async_trait]
impl FillBuilder for UniswapXFillBuilder {
    #[tracing::instrument(skip_all, fields(intent = %intent.id))]
    async fn build(
        &self,
        intent: &Intent,
        plan: &RoutePlan,
        snapshot: &Snapshot,
    ) -> Result<PreparedFill, FillBuilderError> {
        let order = SignedOrder {
            order: intent.raw.clone(),
            sig: intent.signature.clone(),
        };
        let context_hash = keccak256(order.abi_encode());
        let sources = self.sources.build(plan, context_hash, snapshot).await?;

        Ok(PreparedFill::new(
            self.filler,
            fillCall { order, sources }.abi_encode().into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use alloy::primitives::{address, Bytes, B256, U256};
    use async_trait::async_trait;

    use solvent_core::deps::execution::{ExecutionAuthorizer, ExecutionAuthorizerError};
    use solvent_core::primitives::execution::{ExecutionAuthorization, PolicySignature};
    use solvent_core::primitives::registry::AquaEvent;
    use solvent_core::primitives::routing::RouteLeg;
    use solvent_core::primitives::{MakerId, StrategyHash};

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

    fn router() -> Address {
        address!("9999999999999999999999999999999999999999")
    }

    fn builder() -> UniswapXFillBuilder {
        UniswapXFillBuilder::new(
            router(),
            address!("8888888888888888888888888888888888888888"),
            credential(),
            Arc::new(FakeAuthorizer),
        )
    }

    fn credential() -> Address {
        address!("5555555555555555555555555555555555555555")
    }

    fn protected_program(tail: &[u8]) -> Bytes {
        Bytes::from([vec![0x0e, 20], credential().to_vec(), tail.to_vec()].concat())
    }

    fn leg() -> RouteLeg {
        RouteLeg {
            maker: MakerId(Address::from([1u8; 20])),
            strategy_hash: StrategyHash(B256::from([7u8; 32])),
            token_in: address!("6666666666666666666666666666666666666666"),
            token_out: address!("7777777777777777777777777777777777777777"),
            amount_in: U256::from(500u64),
            amount_out: U256::from(1000u64),
        }
    }

    fn snapshot_with(program: Bytes) -> Snapshot {
        let leg = leg();
        let mut snapshot = Snapshot::default();
        snapshot.apply(AquaEvent::Shipped {
            maker: leg.maker,
            app: router(),
            strategy_hash: leg.strategy_hash,
            strategy: program,
        });
        snapshot
    }

    #[tokio::test]
    async fn source_binds_the_routed_strategy_and_amounts() {
        let leg = leg();
        let order = Order {
            maker: leg.maker.0,
            traits: U256::from(3u64),
            data: protected_program(&[0xAB, 0xCD]),
        };
        let snapshot = snapshot_with(Bytes::from(order.abi_encode()));
        let context = B256::from([0xCC; 32]);

        let source = builder()
            .source_for(&leg, 0, context, &snapshot)
            .await
            .expect("maps and authorizes");

        assert_eq!(source.order.maker, order.maker);
        assert_eq!(source.order.traits, order.traits);
        assert_eq!(source.order.data, order.data);
        assert_eq!(source.authorization.contextHash, context);
        assert_eq!(source.authorization.strategyHash, leg.strategy_hash.0);
        assert_eq!(source.authorization.amountOut, leg.amount_out);
        assert_eq!(source.authorization.amountInLimit, leg.amount_in);
        assert_eq!(source.authorization.rebateAmount, U256::ZERO);
        assert_eq!(source.policySignature, Bytes::from(vec![0xAA; 65]));
    }

    #[tokio::test]
    async fn malformed_snapshot_entries_fail_before_signing() {
        let context = B256::from([0xCC; 32]);
        let missing = builder()
            .source_for(&leg(), 0, context, &Snapshot::default())
            .await;
        assert!(matches!(missing, Err(FillBuilderError::MissingStrategy)));

        let malformed = builder()
            .source_for(
                &leg(),
                0,
                context,
                &snapshot_with(Bytes::from(vec![1, 2, 3])),
            )
            .await;
        assert!(matches!(
            malformed,
            Err(FillBuilderError::UndecodableProgram)
        ));

        let order = Order {
            maker: leg().maker.0,
            traits: U256::ZERO,
            data: Bytes::from_static(&[0x11, 0]),
        };
        let unprotected = builder()
            .source_for(
                &leg(),
                0,
                context,
                &snapshot_with(order.abi_encode().into()),
            )
            .await;
        assert!(matches!(
            unprotected,
            Err(FillBuilderError::UnprotectedStrategy)
        ));
    }
}
