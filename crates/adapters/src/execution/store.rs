//! The SQLite fill store (sqlx) behind the `FillStore` port. One row per in-flight fill, keyed by
//! the signed order hash; `track` dedups on it and `untrack` drops a settled fill.

use alloy::primitives::B256;
use async_trait::async_trait;
use solvent_core::deps::execution::{FillStore, FillStoreError};
use solvent_core::primitives::execution::{ExecStatus, TrackedFill};
use solvent_core::primitives::{IntentId, ReservationId};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqlitePool};

pub struct SqliteFillStore {
    pool: SqlitePool,
}

impl SqliteFillStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Apply the embedded schema migrations.
    pub async fn migrate(&self) -> Result<(), FillStoreError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(db)
    }
}

fn db(e: impl std::fmt::Display) -> FillStoreError {
    FillStoreError::Db(e.to_string())
}

fn b256(row: &SqliteRow, col: &str) -> Result<B256, FillStoreError> {
    let bytes: Vec<u8> = row.try_get(col).map_err(db)?;
    B256::try_from(bytes.as_slice()).map_err(|_| db(format!("column '{col}': expected 32 bytes")))
}

fn row_to_tracked(row: &SqliteRow) -> Result<TrackedFill, FillStoreError> {
    Ok(TrackedFill::new(
        IntentId(b256(row, "order_hash")?),
        ReservationId(b256(row, "reservation")?),
    ))
}

#[async_trait]
impl FillStore for SqliteFillStore {
    async fn track(
        &self,
        intent: IntentId,
        reservation: ReservationId,
        handle: &[u8],
    ) -> Result<(), FillStoreError> {
        sqlx::query(
            "INSERT INTO inflight_fill (order_hash, reservation, handle_id) VALUES (?, ?, ?)
             ON CONFLICT (order_hash) DO NOTHING",
        )
        .bind(intent.0.as_slice())
        .bind(reservation.0.as_slice())
        .bind(handle)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn handle(&self, intent: IntentId) -> Result<Option<Vec<u8>>, FillStoreError> {
        sqlx::query_scalar("SELECT handle_id FROM inflight_fill WHERE order_hash = ?")
            .bind(intent.0.as_slice())
            .fetch_optional(&self.pool)
            .await
            .map_err(db)
    }

    async fn untrack(&self, intent: IntentId) -> Result<(), FillStoreError> {
        sqlx::query("DELETE FROM inflight_fill WHERE order_hash = ?")
            .bind(intent.0.as_slice())
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn tracked(&self) -> Result<Vec<TrackedFill>, FillStoreError> {
        sqlx::query("SELECT order_hash, reservation FROM inflight_fill")
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .iter()
            .map(row_to_tracked)
            .collect()
    }

    async fn record_terminal(
        &self,
        intent: IntentId,
        status: &ExecStatus,
    ) -> Result<(), FillStoreError> {
        let (status, tx_hash, block, reason) = match status {
            ExecStatus::Confirmed { tx, block } => (
                "confirmed",
                Some(tx.as_slice().to_vec()),
                Some(*block as i64),
                None,
            ),
            ExecStatus::Failed { reason } => ("failed", None, None, Some(reason.as_str())),
            ExecStatus::Dropped => ("dropped", None, None, None),
            ExecStatus::Pending => return Ok(()),
            _ => return Ok(()),
        };
        sqlx::query(
            "INSERT INTO inflight_fill_terminal (order_hash, status, tx_hash, block, reason)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(order_hash) DO UPDATE SET status = excluded.status,
                 tx_hash = excluded.tx_hash, block = excluded.block, reason = excluded.reason",
        )
        .bind(intent.0.as_slice())
        .bind(status)
        .bind(tx_hash)
        .bind(block)
        .bind(reason)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn terminal(&self, intent: IntentId) -> Result<Option<ExecStatus>, FillStoreError> {
        let row = sqlx::query(
            "SELECT status, tx_hash, block, reason FROM inflight_fill_terminal WHERE order_hash = ?",
        )
        .bind(intent.0.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        let Some(row) = row else { return Ok(None) };
        let status: String = row.try_get("status").map_err(db)?;
        match status.as_str() {
            "confirmed" => {
                let tx = row
                    .try_get::<Option<Vec<u8>>, _>("tx_hash")
                    .map_err(db)?
                    .ok_or_else(|| db("confirmed terminal status has no transaction hash"))?;
                let tx = B256::try_from(tx.as_slice())
                    .map_err(|_| db("terminal tx hash must be 32 bytes"))?;
                let block = row
                    .try_get::<Option<i64>, _>("block")
                    .map_err(db)?
                    .ok_or_else(|| db("confirmed terminal status has no block"))?;
                let block = u64::try_from(block).map_err(|_| db("terminal block is negative"))?;
                Ok(Some(ExecStatus::Confirmed { tx, block }))
            }
            "failed" => Ok(Some(ExecStatus::Failed {
                reason: row
                    .try_get::<Option<String>, _>("reason")
                    .map_err(db)?
                    .unwrap_or_else(|| "execution failed".to_string()),
            })),
            "dropped" => Ok(Some(ExecStatus::Dropped)),
            other => Err(db(format!("unknown terminal status '{other}'"))),
        }
    }
}
