//! Durable rebate batch restart and idempotency checks over an in-memory SQLite database.

use std::collections::BTreeMap;

use std::sync::Arc;

use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::sol;
use alloy::sol_types::SolValue;
use async_trait::async_trait;
use solvent_adapters::ledger::SqliteLedgerStore;
use solvent_adapters::rebate::{
    FillerRebateCallBuilder, SqliteRebateAccrualSource, SqliteRebateStore,
};
use solvent_core::deps::execution::{ExecutionAuthorizer, ExecutionAuthorizerError};
use solvent_core::deps::rebate::{
    ExecutedRebateCursor, ExecutedRebateQuery, RebateAccrualSource, RebateCallBuilder, RebateStore,
    RebateStoreError,
};
use solvent_core::primitives::execution::{ExecutionAuthorization, PolicySignature};
use solvent_core::primitives::rebate::{
    RebateAccrual, RebateAllocation, RebateBatch, RebateBatchState, RebateExecutedEvent,
    RebatePlan, RebateSettlement,
};
use solvent_core::primitives::registry::{MakerStrategy, StrategyKey, TokenPair};
use solvent_core::primitives::trade::TradeId;
use solvent_core::primitives::{ChainId, MakerId, RebateBatchId, ReservationId, StrategyHash};
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
        1,
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
            2,
        ),
    );
    assert!(matches!(
        restarted.save(&different_id).await,
        Err(RebateStoreError::Conflict(hash)) if hash == strategy().strategy_hash
    ));
}

#[tokio::test]
async fn execution_marker_closes_the_batch_and_cursor_never_rewinds() {
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
    store.save(&batch).await.unwrap();
    let RebateBatchState::Ready(execution) = &batch.state else {
        panic!("fixture must be ready");
    };
    let plan = execution.plan.as_ref();
    let event = RebateExecutedEvent::new(
        plan.batch_id,
        plan.strategy.strategy_hash,
        token(9),
        plan.strategy.maker,
        plan.token_in,
        plan.token_out,
        plan.amount_in,
        plan.amount_out,
        plan.maker_rebate,
        B256::from([8; 32]),
        120,
        3,
    );
    let settlement = RebateSettlement::new(plan.clone(), execution.reservation, event, 1_000);

    store.begin_settlement(&settlement).await.unwrap();
    let replay = RebateSettlement::new(
        plan.clone(),
        execution.reservation,
        settlement.event.clone(),
        1_001,
    );
    store.begin_settlement(&replay).await.unwrap();
    assert_eq!(
        store.pending_settlements().await.unwrap(),
        vec![settlement.clone()]
    );

    store.finish_settlement(batch.id).await.unwrap();
    store.finish_settlement(batch.id).await.unwrap();
    assert!(store.open_batches().await.unwrap().is_empty());
    assert!(store.pending_settlements().await.unwrap().is_empty());
    let status: (String,) =
        sqlx::query_as("SELECT status FROM rebate_settlement WHERE batch_id = ?")
            .bind(batch.id.0.to_vec())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status.0, "executed");
    assert_eq!(
        store.executed_rebate(batch.id).await.unwrap(),
        Some(settlement.clone())
    );
    assert_eq!(
        store
            .executed_rebates(&ExecutedRebateQuery::new(None, None, 10))
            .await
            .unwrap(),
        vec![settlement.clone()]
    );
    assert!(store
        .executed_rebates(&ExecutedRebateQuery::new(Some(MakerId(token(2))), None, 10,))
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .executed_rebates(&ExecutedRebateQuery::new(
            None,
            Some(ExecutedRebateCursor::new(settlement.executed_at, batch.id,)),
            10,
        ))
        .await
        .unwrap()
        .is_empty());

    let chain = ChainId(31337);
    assert_eq!(store.scan_cursor(chain).await.unwrap(), None);
    store.save_scan_cursor(chain, 120).await.unwrap();
    store.save_scan_cursor(chain, 100).await.unwrap();
    assert_eq!(store.scan_cursor(chain).await.unwrap(), Some(120));
}

#[tokio::test]
async fn confirmed_trade_projection_is_idempotent_and_uses_canonical_token_weight() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    SqliteLedgerStore::new(pool.clone())
        .migrate()
        .await
        .expect("migrate schema");
    let trade_id = TradeId(Ulid::from_parts(4, 1));
    sqlx::query(
        "INSERT INTO trade
         (id, order_hash, taker, token_in, token_out, amount_in, min_amount_out,
          status, status_rank, deadline_block, block_number, created_at, settled_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'confirmed', 5, 100, 42, 1, 2)",
    )
    .bind(trade_id.to_string())
    .bind(B256::from([1; 32]).to_vec())
    .bind(token(8).to_vec())
    .bind(token(5).to_vec())
    .bind(token(4).to_vec())
    .bind("12")
    .bind("34")
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO trade_leg
         (trade_id, idx, maker, strategy_hash, amount_in, amount_out)
         VALUES (?, 0, ?, ?, '12', '34')",
    )
    .bind(trade_id.to_string())
    .bind(strategy().maker.0.to_vec())
    .bind(strategy().strategy_hash.0.to_vec())
    .execute(&pool)
    .await
    .unwrap();
    let source = SqliteRebateAccrualSource::new(pool.clone(), strategy().app);

    let pending = source.pending(10).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].allocation_weight, U256::from(34u64));
    assert_eq!(pending[0].confirmed_block, 42);

    let batch = RebateBatch::new(
        RebateBatchId(B256::from([2; 32])),
        TokenPair::new(token(4), token(5)),
        pending[0].clone(),
    );
    SqliteRebateStore::new(pool.clone())
        .save(&batch)
        .await
        .unwrap();
    assert!(source.pending(10).await.unwrap().is_empty());

    let deferred_trade_id = TradeId(Ulid::from_parts(5, 1));
    sqlx::query(
        "INSERT INTO trade
         (id, order_hash, taker, token_in, token_out, amount_in, min_amount_out,
          status, status_rank, deadline_block, block_number, created_at, settled_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'confirmed', 5, 100, 43, 2, 3)",
    )
    .bind(deferred_trade_id.to_string())
    .bind(B256::from([2; 32]).to_vec())
    .bind(token(8).to_vec())
    .bind(token(5).to_vec())
    .bind(token(4).to_vec())
    .bind("13")
    .bind("35")
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO trade_leg
         (trade_id, idx, maker, strategy_hash, amount_in, amount_out)
         VALUES (?, 0, ?, ?, '13', '35')",
    )
    .bind(deferred_trade_id.to_string())
    .bind(strategy().maker.0.to_vec())
    .bind(strategy().strategy_hash.0.to_vec())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE rebate_batch SET state = 'ready' WHERE id = ?")
        .bind(batch.id.0.to_vec())
        .execute(&pool)
        .await
        .unwrap();
    assert!(source.pending(10).await.unwrap().is_empty());

    SqliteRebateStore::new(pool.clone())
        .close(batch.id)
        .await
        .unwrap();
    let deferred = source.pending(10).await.unwrap();
    assert_eq!(deferred.len(), 1);
    assert_eq!(deferred[0].trade_id, deferred_trade_id);

    sqlx::query(
        "UPDATE trade SET status = 'submitted', status_rank = 4, settled_at = NULL WHERE id = ?",
    )
    .bind(trade_id.to_string())
    .execute(&pool)
    .await
    .unwrap();
    assert!(source.has_unsettled(&strategy()).await.unwrap());
}
