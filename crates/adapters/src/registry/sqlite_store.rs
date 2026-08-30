//! The SQLite registry store (sqlx) behind the `Store` port. Append-only event log keyed by
//! `(chain, block, block_hash, log_index)`; `insert` is idempotent and returns only the new rows.

use async_trait::async_trait;
use solvent_core::{
    deps::registry::{Store, StoreError},
    primitives::{
        registry::{AquaEvent, EventCursor, EventExt},
        ChainId,
    },
};
use sqlx::{types::Json, SqlitePool};

pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Apply the embedded schema migrations.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(db)
    }
}

/// SQLite integers are signed 64-bit; block/log/chain values are non-negative and
/// well within `i64`, but convert with a check rather than a silent `as` cast.
fn i64_of(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Db(format!("value {value} exceeds i64")))
}

fn db(e: impl std::fmt::Display) -> StoreError {
    StoreError::Db(e.to_string())
}

#[async_trait]
impl Store for SqliteStore {
    async fn cursor(&self, chain: ChainId) -> Result<Option<EventCursor>, StoreError> {
        let row: Option<(i64, i64)> =
            sqlx::query_as("SELECT block_number, log_index FROM registry_cursor WHERE chain = ?")
                .bind(i64_of(chain.0)?)
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?;
        Ok(row.map(|(block_number, log_index)| EventCursor {
            block_number: block_number as u64,
            log_index: log_index as u64,
        }))
    }

    async fn insert(
        &self,
        chain: ChainId,
        events: &[EventExt<AquaEvent>],
    ) -> Result<Vec<EventExt<AquaEvent>>, StoreError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }

        // SQLite has no array/`UNNEST` binding, so insert each event within one
        // transaction. `ON CONFLICT DO NOTHING RETURNING` yields a row only when the
        // insert actually happened, so a re-scanned overlap contributes nothing;
        // input order is fold order, so a `Shipped` still precedes its `Pushed`.
        let chain_id = i64_of(chain.0)?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        let mut inserted = Vec::with_capacity(events.len());
        for event in events {
            let cursor = event.cursor().ok_or(StoreError::Unpositioned)?;
            let block_hash = event.block_hash.ok_or(StoreError::Unpositioned)?;
            let added: Option<Json<EventExt<AquaEvent>>> = sqlx::query_scalar(
                "INSERT INTO aqua_event (chain, block_number, block_hash, log_index, event)
                 VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT DO NOTHING
                 RETURNING event",
            )
            .bind(chain_id)
            .bind(i64_of(cursor.block_number)?)
            .bind(block_hash.to_vec())
            .bind(i64_of(cursor.log_index)?)
            .bind(Json(event))
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if let Some(Json(event)) = added {
                inserted.push(event);
            }
        }
        tx.commit().await.map_err(db)?;
        Ok(inserted)
    }

    async fn save_cursor(&self, chain: ChainId, cursor: EventCursor) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO registry_cursor (chain, block_number, log_index)
             VALUES (?, ?, ?)
             ON CONFLICT (chain) DO UPDATE
                 SET block_number = excluded.block_number, log_index = excluded.log_index",
        )
        .bind(i64_of(chain.0)?)
        .bind(i64_of(cursor.block_number)?)
        .bind(i64_of(cursor.log_index)?)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn events(&self, chain: ChainId) -> Result<Vec<EventExt<AquaEvent>>, StoreError> {
        let rows: Vec<Json<EventExt<AquaEvent>>> = sqlx::query_scalar(
            "SELECT event FROM aqua_event WHERE chain = ? ORDER BY block_number, log_index",
        )
        .bind(i64_of(chain.0)?)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows.into_iter().map(|Json(event)| event).collect())
    }
}
