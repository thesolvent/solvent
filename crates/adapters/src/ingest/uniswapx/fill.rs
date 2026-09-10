//! Builds policy-authorized `UniswapXAquaFiller.fill(...)` calldata from a routed plan.

use std::sync::Arc;

use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::sol_types::{SolCall, SolValue};
use async_trait::async_trait;

use solvent_core::deps::execution::ExecutionAuthorizer;
use solvent_core::deps::ingest::{FillBuilder, FillBuilderError};
use solvent_core::primitives::execution::{ExecutionAuthorization, UserFillAuthorization};
use solvent_core::primitives::ingest::Intent;
use solvent_core::primitives::registry::{Snapshot, StrategyKey};
use solvent_core::primitives::routing::{RouteLeg, RoutePlan};

use crate::execution::filler::{fillCall, Authorization, Order, SignedOrder, SourceSwap};

/// The router is the deployment's Aqua app and hashes each source strategy. The authorizer binds
/// the exact routed amounts and signed UniswapX order before the filler can execute them.
pub struct UniswapXFillBuilder {
    router: Address,
    authorizer: Arc<dyn ExecutionAuthorizer>,
}

impl UniswapXFillBuilder {
    pub fn new(router: Address, authorizer: Arc<dyn ExecutionAuthorizer>) -> Self {
        Self { router, authorizer }
    }

    async fn source_for(
        &self,
        leg: &RouteLeg,
        index: usize,
        context_hash: B256,
        snapshot: &Snapshot,
    ) -> Result<SourceSwap, FillBuilderError> {
        let key = StrategyKey {
            maker: leg.maker,
            app: self.router,
            strategy_hash: leg.strategy_hash,
        };
        let strategy = snapshot
            .strategy(&key)
            .ok_or(FillBuilderError::MissingStrategy)?;
        let order = Order::abi_decode(&strategy.program)
            .map_err(|_| FillBuilderError::UndecodableProgram)?;
        if order.maker != leg.maker.0 {
            return Err(FillBuilderError::StrategyMakerMismatch);
        }

        let authorization = ExecutionAuthorization::user_fill(UserFillAuthorization {
            nonce: user_fill_nonce(context_hash, leg, index),
            context_hash,
            strategy_hash: leg.strategy_hash,
            maker: leg.maker,
            token_in: leg.token_in,
            token_out: leg.token_out,
            amount_out: leg.amount_out,
            amount_in_limit: leg.amount_in,
        });
        let policy_signature = self
            .authorizer
            .authorize(&authorization)
            .await?
            .into_bytes();

        Ok(SourceSwap {
            order,
            authorization: Authorization::from(&authorization),
            policySignature: policy_signature,
        })
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
    ) -> Result<Bytes, FillBuilderError> {
        if plan.legs.is_empty() {
            return Err(FillBuilderError::NoLegs);
        }

        let order = SignedOrder {
            order: intent.raw.clone(),
            sig: intent.signature.clone(),
        };
        let context_hash = keccak256(order.abi_encode());
        let mut sources = Vec::with_capacity(plan.legs.len());
        for (index, leg) in plan.legs.iter().enumerate() {
            sources.push(self.source_for(leg, index, context_hash, snapshot).await?);
        }

        Ok(Bytes::from(fillCall { order, sources }.abi_encode()))
    }
}

fn user_fill_nonce(context_hash: B256, leg: &RouteLeg, index: usize) -> U256 {
    let digest = keccak256(
        (
            context_hash,
            leg.strategy_hash.0,
            leg.maker.0,
            leg.token_in,
            leg.token_out,
            leg.amount_in,
            leg.amount_out,
            U256::from(index),
        )
            .abi_encode(),
    );
    U256::from_be_slice(digest.as_slice())
}

#[cfg(test)]
mod tests {
    use alloy::primitives::{address, B256, U256};
    use async_trait::async_trait;

    use solvent_core::deps::execution::{ExecutionAuthorizer, ExecutionAuthorizerError};
    use solvent_core::primitives::execution::{ExecutionAuthorization, PolicySignature};
    use solvent_core::primitives::registry::AquaEvent;
    use solvent_core::primitives::{MakerId, StrategyHash};

    use super::*;

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
        UniswapXFillBuilder::new(router(), Arc::new(FakeAuthorizer))
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
            data: Bytes::from(vec![0xAB, 0xCD]),
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
    }
}
