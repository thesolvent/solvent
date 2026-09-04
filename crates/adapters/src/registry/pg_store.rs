//! The Postgres registry store (sqlx). The event log is append-only, keyed by
//! `(chain, block_number, block_hash, log_index)`; `insert` adds events
//! idempotently and reports the rows it actually added, so the caller folds each
//! event exactly once. `save_cursor` records scan progress separately, written
//! last in a cycle.

use async_trait::async_trait;
use solvent_core::{
    deps::registry::{Store, StoreError},
    primitives::{
        registry::{AquaEvent, EventCursor, EventExt},
        ChainId,
    },
};
use sqlx::{types::Json, PgPool};

pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    pub fn new(pool: PgPool) -> Self {
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

/// Postgres has no unsigned integer; block/log/chain values are non-negative and
/// well within `i64`, but convert with a check rather than a silent `as` cast.
fn i64_of(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Db(format!("value {value} exceeds i64")))
}

fn db(e: impl std::fmt::Display) -> StoreError {
    StoreError::Db(e.to_string())
}

#[async_trait]
impl Store for PgStore {
    async fn cursor(&self, chain: ChainId) -> Result<Option<EventCursor>, StoreError> {
        let row: Option<(i64, i64)> =
            sqlx::query_as("SELECT block_number, log_index FROM registry_cursor WHERE chain = $1")
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

        // Pivot the rows into per-column arrays for one bulk `UNNEST` insert.
        let mut block_numbers = Vec::with_capacity(events.len());
        let mut block_hashes = Vec::with_capacity(events.len());
        let mut log_indexes = Vec::with_capacity(events.len());
        let mut payloads = Vec::with_capacity(events.len());
        for event in events {
            let cursor = event.cursor().ok_or(StoreError::Unpositioned)?;
            let block_hash = event.block_hash.ok_or(StoreError::Unpositioned)?;
            block_numbers.push(i64_of(cursor.block_number)?);
            block_hashes.push(block_hash.to_vec());
            log_indexes.push(i64_of(cursor.log_index)?);
            payloads.push(serde_json::to_value(event).map_err(db)?);
        }

        // Bulk-insert idempotently; `RETURNING` yields only the rows actually
        // added, re-ordered to fold order so a `Shipped` precedes its `Pushed`.
        let inserted: Vec<Json<EventExt<AquaEvent>>> = sqlx::query_scalar(
            "WITH added AS (
                 INSERT INTO aqua_event (chain, block_number, block_hash, log_index, event)
                 SELECT $1, block_number, block_hash, log_index, event
                 FROM UNNEST($2::bigint[], $3::bytea[], $4::bigint[], $5::jsonb[])
                     AS t(block_number, block_hash, log_index, event)
                 ON CONFLICT DO NOTHING
                 RETURNING event, block_number, log_index
             )
             SELECT event FROM added ORDER BY block_number, log_index",
        )
        .bind(i64_of(chain.0)?)
        .bind(block_numbers)
        .bind(block_hashes)
        .bind(log_indexes)
        .bind(payloads)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(inserted.into_iter().map(|Json(event)| event).collect())
    }

    async fn save_cursor(&self, chain: ChainId, cursor: EventCursor) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO registry_cursor (chain, block_number, log_index)
             VALUES ($1, $2, $3)
             ON CONFLICT (chain) DO UPDATE SET block_number = $2, log_index = $3",
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
            "SELECT event FROM aqua_event WHERE chain = $1 ORDER BY block_number, log_index",
        )
        .bind(i64_of(chain.0)?)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows.into_iter().map(|Json(event)| event).collect())
    }
}
