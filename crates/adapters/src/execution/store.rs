//! The SQLite fill store (sqlx) behind the `FillStore` port. One row per in-flight fill, keyed by
//! the signed order hash; `track` dedups on it and `untrack` drops a settled fill.

use alloy::primitives::B256;
use async_trait::async_trait;
use solvent_core::deps::execution::{FillStore, FillStoreError};
use solvent_core::primitives::execution::TrackedFill;
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
}
