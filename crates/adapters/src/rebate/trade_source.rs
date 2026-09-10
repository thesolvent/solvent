//! Confirmed trade legs projected from the existing trade tables into rebate accruals.

use std::str::FromStr;

use alloy::primitives::{Address, B256, U256};
use async_trait::async_trait;
use solvent_core::deps::rebate::{RebateAccrualSource, RebateAccrualSourceError};
use solvent_core::primitives::rebate::RebateAccrual;
use solvent_core::primitives::registry::{StrategyKey, TokenPair};
use solvent_core::primitives::trade::TradeId;
use solvent_core::primitives::{MakerId, StrategyHash};
use sqlx::{FromRow, SqlitePool};

pub struct SqliteRebateAccrualSource {
    pool: SqlitePool,
    app: Address,
}

impl SqliteRebateAccrualSource {
    pub fn new(pool: SqlitePool, app: Address) -> Self {
        Self { pool, app }
    }
}

#[derive(FromRow)]
struct PendingRow {
    trade_id: String,
    token_in: Vec<u8>,
    token_out: Vec<u8>,
    block_number: i64,
    maker: Vec<u8>,
    strategy_hash: Vec<u8>,
    amount_in: String,
    amount_out: String,
}

impl PendingRow {
    fn into_domain(self, app: Address) -> Result<RebateAccrual, RebateAccrualSourceError> {
        let token_in = address(&self.token_in, "token_in")?;
        let token_out = address(&self.token_out, "token_out")?;
        let amount_in = amount(&self.amount_in, "amount_in")?;
        let amount_out = amount(&self.amount_out, "amount_out")?;
        let pair = TokenPair::new(token_in, token_out);
        let allocation_weight = if token_in == pair.lo {
            amount_in
        } else {
            amount_out
        };
        Ok(RebateAccrual::new(
            TradeId::from_str(&self.trade_id).map_err(db)?,
            StrategyKey {
                maker: MakerId(address(&self.maker, "maker")?),
                app,
                strategy_hash: StrategyHash(hash(&self.strategy_hash, "strategy_hash")?),
            },
            token_in,
            token_out,
            amount_in,
            amount_out,
            allocation_weight,
            u64::try_from(self.block_number).map_err(|_| db("block_number cannot be negative"))?,
        ))
    }
}

#[async_trait]
impl RebateAccrualSource for SqliteRebateAccrualSource {
    async fn pending(&self, limit: u32) -> Result<Vec<RebateAccrual>, RebateAccrualSourceError> {
        // Later fills stay in the trade tables until the current public authorization is closed.
        let rows: Vec<PendingRow> = sqlx::query_as(
            "SELECT t.id AS trade_id, t.token_in, t.token_out, t.block_number,
                    l.maker, l.strategy_hash, l.amount_in, l.amount_out
             FROM trade t
             JOIN trade_leg l ON l.trade_id = t.id
             WHERE t.status = 'confirmed' AND t.block_number IS NOT NULL
               AND NOT EXISTS (
                   SELECT 1 FROM rebate_accrual a
                   WHERE a.maker = l.maker AND a.app = ?
                     AND a.strategy_hash = l.strategy_hash AND a.trade_id = t.id
               )
               AND NOT EXISTS (
                   SELECT 1 FROM rebate_batch b
                   WHERE b.maker = l.maker AND b.app = ?
                     AND b.strategy_hash = l.strategy_hash AND b.is_open = 1
                     AND b.state = 'ready'
               )
             ORDER BY t.id, l.idx LIMIT ?",
        )
        .bind(self.app.to_vec())
        .bind(self.app.to_vec())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|row| row.into_domain(self.app))
            .collect()
    }

    async fn has_unsettled(
        &self,
        strategy: &StrategyKey,
    ) -> Result<bool, RebateAccrualSourceError> {
        let row: (i64,) = sqlx::query_as(
            "SELECT EXISTS (
                 SELECT 1 FROM trade t JOIN trade_leg l ON l.trade_id = t.id
                 WHERE l.maker = ? AND l.strategy_hash = ? AND t.settled_at IS NULL
                   AND t.status IN ('reserved', 'simulated', 'submitted')
             )",
        )
        .bind(strategy.maker.0.to_vec())
        .bind(strategy.strategy_hash.0.to_vec())
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        Ok(row.0 != 0)
    }
}

fn amount(value: &str, field: &str) -> Result<U256, RebateAccrualSourceError> {
    U256::from_str_radix(value, 10).map_err(|_| db(format!("{field} is not a uint256")))
}

fn address(value: &[u8], field: &str) -> Result<Address, RebateAccrualSourceError> {
    Address::try_from(value).map_err(|_| db(format!("{field} must contain 20 bytes")))
}

fn hash(value: &[u8], field: &str) -> Result<B256, RebateAccrualSourceError> {
    B256::try_from(value).map_err(|_| db(format!("{field} must contain 32 bytes")))
}

fn db(error: impl std::fmt::Display) -> RebateAccrualSourceError {
    RebateAccrualSourceError::Db(error.to_string())
}
