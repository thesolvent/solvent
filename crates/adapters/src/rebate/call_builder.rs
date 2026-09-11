//! Signed `executeRebate` calldata construction for the protected filler contract.

use std::sync::Arc;

use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::sol_types::{SolCall, SolValue};
use async_trait::async_trait;

use solvent_core::deps::execution::ExecutionAuthorizer;
use solvent_core::deps::rebate::{RebateCallBuilder, RebateCallBuilderError};
use solvent_core::primitives::execution::{ExecutionAuthorization, RebateAuthorization};
use solvent_core::primitives::rebate::{RebateExecution, RebatePlan};
use solvent_core::primitives::registry::MakerStrategy;
use solvent_core::primitives::ReservationId;

use crate::execution::filler::{executeRebateCall, uses_taker_credential, Authorization, Order};

pub struct FillerRebateCallBuilder {
    taker_credential: Address,
    authorizer: Arc<dyn ExecutionAuthorizer>,
}

impl FillerRebateCallBuilder {
    pub fn new(taker_credential: Address, authorizer: Arc<dyn ExecutionAuthorizer>) -> Self {
        Self {
            taker_credential,
            authorizer,
        }
    }
}

#[async_trait]
impl RebateCallBuilder for FillerRebateCallBuilder {
    #[tracing::instrument(skip_all, fields(batch = %plan.batch_id))]
    async fn build(
        &self,
        strategy: &MakerStrategy,
        plan: RebatePlan,
        reservation: ReservationId,
        deadline_block: u64,
        published_at: u64,
    ) -> Result<RebateExecution, RebateCallBuilderError> {
        if plan.strategy != strategy.key {
            return Err(RebateCallBuilderError::StrategyMismatch);
        }
        validate_plan(&plan, deadline_block)?;
        let order = Order::abi_decode(&strategy.program)
            .map_err(|_| RebateCallBuilderError::UndecodableProgram)?;
        if order.maker != strategy.key.maker.0 {
            return Err(RebateCallBuilderError::StrategyMakerMismatch);
        }
        if !uses_taker_credential(&order.data, self.taker_credential) {
            return Err(RebateCallBuilderError::UnprotectedStrategy);
        }
        if keccak256(order.abi_encode()) != strategy.key.strategy_hash.0 {
            return Err(RebateCallBuilderError::StrategyHashMismatch);
        }

        let authorization = ExecutionAuthorization::rebate(RebateAuthorization {
            nonce: rebate_nonce(&plan, deadline_block),
            context_hash: plan.batch_id.0,
            strategy_hash: strategy.key.strategy_hash,
            maker: strategy.key.maker,
            token_in: plan.token_in,
            token_out: plan.token_out,
            amount_out: plan.amount_out,
            amount_in_limit: plan.amount_in,
            rebate_amount: plan.maker_rebate,
            deadline_block,
        });
        let policy_signature = self
            .authorizer
            .authorize(&authorization)
            .await
            .map_err(|_| RebateCallBuilderError::Signing)?;
        let calldata = Bytes::from(
            executeRebateCall {
                order: order.clone(),
                authorization: Authorization::from(&authorization),
                signature: policy_signature.as_bytes().clone(),
            }
            .abi_encode(),
        );

        Ok(RebateExecution::new(
            plan,
            reservation,
            authorization,
            Bytes::from(order.abi_encode()),
            policy_signature,
            calldata,
            published_at,
        ))
    }
}

fn validate_plan(plan: &RebatePlan, deadline_block: u64) -> Result<(), RebateCallBuilderError> {
    if plan.batch_id.0 == B256::ZERO {
        return Err(RebateCallBuilderError::InvalidPlan("batch id is zero"));
    }
    if plan.strategy.maker.0 == Address::ZERO {
        return Err(RebateCallBuilderError::InvalidPlan("maker is zero"));
    }
    if plan.token_in == Address::ZERO
        || plan.token_out == Address::ZERO
        || plan.token_in == plan.token_out
    {
        return Err(RebateCallBuilderError::InvalidPlan("token pair is invalid"));
    }
    if plan.amount_in.is_zero() || plan.amount_out.is_zero() || plan.maker_rebate.is_zero() {
        return Err(RebateCallBuilderError::InvalidPlan(
            "amounts and maker rebate must be non-zero",
        ));
    }
    if plan.amount_in.checked_add(plan.maker_rebate).is_none() {
        return Err(RebateCallBuilderError::InvalidPlan(
            "executor deposit exceeds uint256",
        ));
    }
    if deadline_block == 0 {
        return Err(RebateCallBuilderError::InvalidPlan(
            "deadline block is zero",
        ));
    }
    Ok(())
}

fn rebate_nonce(plan: &RebatePlan, deadline_block: u64) -> U256 {
    U256::from_be_slice(
        keccak256(
            (
                plan.batch_id.0,
                plan.strategy.strategy_hash.0,
                plan.strategy.maker.0,
                plan.token_in,
                plan.token_out,
                plan.amount_in,
                plan.amount_out,
                plan.maker_rebate,
                deadline_block,
            )
                .abi_encode(),
        )
        .as_slice(),
    )
}

#[cfg(test)]
mod tests {
    use alloy::primitives::{Address, B256};
    use async_trait::async_trait;

    use solvent_core::deps::execution::{ExecutionAuthorizer, ExecutionAuthorizerError};
    use solvent_core::primitives::execution::PolicySignature;
    use solvent_core::primitives::rebate::RebateAllocation;
    use solvent_core::primitives::registry::StrategyKey;
    use solvent_core::primitives::trade::TradeId;
    use solvent_core::primitives::{MakerId, RebateBatchId, StrategyHash};
    use ulid::Ulid;

    use super::*;

    struct FixedSigner;

    #[async_trait]
    impl ExecutionAuthorizer for FixedSigner {
        async fn authorize(
            &self,
            _authorization: &ExecutionAuthorization,
        ) -> Result<PolicySignature, ExecutionAuthorizerError> {
            Ok(PolicySignature::new(Bytes::from(vec![0xAA; 65])))
        }
    }

    fn credential() -> Address {
        Address::from([3; 20])
    }

    fn protected_program(tail: &[u8]) -> Bytes {
        Bytes::from([vec![0x0e, 20], credential().to_vec(), tail.to_vec()].concat())
    }

    #[tokio::test]
    async fn calldata_binds_the_plan_and_reuses_its_stable_batch_context() {
        let order = Order {
            maker: Address::from([1; 20]),
            traits: U256::from(4u64),
            data: protected_program(&[5, 6]),
        };
        let key = StrategyKey {
            maker: MakerId(Address::from([1; 20])),
            app: Address::from([2; 20]),
            strategy_hash: StrategyHash(keccak256(order.abi_encode())),
        };
        let strategy = MakerStrategy::new(key, &order.abi_encode());
        let batch_id = RebateBatchId(B256::from([7; 32]));
        let plan = RebatePlan::new(
            batch_id,
            key,
            Address::from([8; 20]),
            Address::from([9; 20]),
            U256::from(100u64),
            U256::from(110u64),
            U256::from(10u64),
            U256::from(2u64),
            U256::from(7u64),
            U256::from(1u64),
            250,
            vec![RebateAllocation::new(
                TradeId(Ulid::from_parts(1, 1)),
                U256::from(7u64),
            )],
        );
        let builder = FillerRebateCallBuilder::new(credential(), Arc::new(FixedSigner));

        let execution = builder
            .build(
                &strategy,
                plan.clone(),
                ReservationId(B256::from([10; 32])),
                1_500,
                1_000,
            )
            .await
            .unwrap();
        let decoded = executeRebateCall::abi_decode(&execution.calldata).unwrap();

        assert_eq!(execution.authorization.context_hash, batch_id.0);
        assert_eq!(execution.authorization.amount_in_limit, plan.amount_in);
        assert_eq!(execution.authorization.amount_out, plan.amount_out);
        assert_eq!(execution.authorization.rebate_amount, plan.maker_rebate);
        assert_eq!(decoded.order.maker, order.maker);
        assert_eq!(decoded.order.traits, order.traits);
        assert_eq!(decoded.order.data, order.data);
        assert_eq!(decoded.authorization.contextHash, batch_id.0);
        assert_eq!(decoded.signature, Bytes::from(vec![0xAA; 65]));

        let wrong_credential =
            FillerRebateCallBuilder::new(Address::from([4; 20]), Arc::new(FixedSigner))
                .build(
                    &strategy,
                    plan,
                    ReservationId(B256::from([10; 32])),
                    1_500,
                    1_000,
                )
                .await;
        assert!(matches!(
            wrong_credential,
            Err(RebateCallBuilderError::UnprotectedStrategy)
        ));
    }
}
