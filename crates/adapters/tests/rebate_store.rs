//! Durable rebate batch restart and idempotency checks over an in-memory SQLite database.

use std::collections::BTreeMap;

use std::sync::Arc;

use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::sol;
use alloy::sol_types::SolValue;
use async_trait::async_trait;
use solvent_adapters::ledger::SqliteLedgerStore;
use solvent_adapters::rebate::{FillerRebateCallBuilder, SqliteRebateStore};
use solvent_core::deps::execution::{ExecutionAuthorizer, ExecutionAuthorizerError};
use solvent_core::deps::rebate::{RebateCallBuilder, RebateStore, RebateStoreError};
use solvent_core::primitives::execution::{ExecutionAuthorization, PolicySignature};
use solvent_core::primitives::rebate::{
    RebateAccrual, RebateAllocation, RebateBatch, RebateBatchState, RebatePlan,
};
use solvent_core::primitives::registry::{MakerStrategy, StrategyKey, TokenPair};
use solvent_core::primitives::trade::TradeId;
use solvent_core::primitives::{MakerId, RebateBatchId, ReservationId, StrategyHash};
use sqlx::sqlite::SqlitePoolOptions;
use ulid::Ulid;

sol! {
    struct TestOrder {
        address maker;
        uint256 traits;
        bytes data;
    }
}

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

fn token(value: u8) -> Address {
    Address::from([value; 20])
}

fn order() -> TestOrder {
    TestOrder {
        maker: token(1),
        traits: U256::from(4u64),
        data: Bytes::from([vec![0x0e, 20], token(3).to_vec(), vec![5, 6]].concat()),
    }
}

fn strategy() -> StrategyKey {
    let order = order();
    StrategyKey {
        maker: MakerId(token(1)),
        app: token(2),
        strategy_hash: StrategyHash(keccak256(order.abi_encode())),
    }
}

fn accrual(amount_in: u64) -> RebateAccrual {
    RebateAccrual::new(
        TradeId(Ulid::from_parts(1, 1)),
        strategy(),
        token(4),
        token(5),
        U256::from(amount_in),
        U256::from(9u64),
        U256::from(10u64),
    )
}

async fn ready_batch() -> RebateBatch {
    let id = RebateBatchId(B256::from([6; 32]));
    let reservation = ReservationId(B256::from([7; 32]));
    let trade_id = accrual(10).trade_id;
    let plan = RebatePlan::new(
        id,
        strategy(),
        token(5),
        token(4),
        U256::from(100u64),
        U256::from(110u64),
        U256::from(10u64),
        U256::from(2u64),
        U256::from(7u64),
        U256::from(1u64),
        250,
        vec![RebateAllocation::new(trade_id, U256::from(7u64))],
    );
    let order = order();
    let maker_strategy = MakerStrategy::new(strategy(), &order.abi_encode());
    let execution = FillerRebateCallBuilder::new(token(3), Arc::new(FixedSigner))
        .build(&maker_strategy, plan, reservation, 1_500, 1_000)
        .await
        .expect("build fixture calldata");
    RebateBatch::from_parts(
        id,
        strategy(),
        TokenPair::new(token(4), token(5)),
        BTreeMap::from([(trade_id, accrual(10))]),
        RebateBatchState::Ready(Box::new(execution)),
    )
}

#[tokio::test]
async fn restart_preserves_one_idempotent_batch_and_exact_executable_calldata() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    SqliteLedgerStore::new(pool.clone())
        .migrate()
        .await
        .expect("migrate schema");
    let store = SqliteRebateStore::new(pool.clone());
    let batch = ready_batch().await;
    let expected_calldata = match &batch.state {
        RebateBatchState::Ready(execution) => execution.calldata.clone(),
        _ => panic!("fixture must be ready"),
    };

    store.save(&batch).await.unwrap();
    store.save(&batch).await.unwrap();

    let restarted = SqliteRebateStore::new(pool);
    let recovered = restarted.open_batches().await.unwrap();
    assert_eq!(recovered, vec![batch.clone()]);
    let RebateBatchState::Ready(execution) = &recovered[0].state else {
        panic!("ready batch must recover as ready");
    };
    assert_eq!(execution.calldata, expected_calldata);
    assert_ne!(execution.authorization.nonce, U256::ZERO);

    let mut conflicting_leg = batch.clone();
    conflicting_leg
        .accruals
        .insert(accrual(11).trade_id, accrual(11));
    assert!(matches!(
        restarted.save(&conflicting_leg).await,
        Err(RebateStoreError::Conflict(hash)) if hash == strategy().strategy_hash
    ));

    let different_id = RebateBatch::new(
        RebateBatchId(B256::from([9; 32])),
        batch.pair,
        RebateAccrual::new(
            TradeId(Ulid::from_parts(2, 2)),
            strategy(),
            token(4),
            token(5),
            U256::from(1u64),
            U256::from(1u64),
            U256::from(1u64),
        ),
    );
    assert!(matches!(
        restarted.save(&different_id).await,
        Err(RebateStoreError::Conflict(hash)) if hash == strategy().strategy_hash
    ));
}
