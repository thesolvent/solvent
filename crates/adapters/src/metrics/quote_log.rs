//! SQLite quote log: one `quote_event` row per served quote plus a `quote_participant` row per
//! sourced strategy, written in one transaction. The clock stamps `served_at`.

use std::sync::Arc;

use async_trait::async_trait;
use solvent_core::deps::ledger::Clock;
use solvent_core::deps::quote_log::{QuoteLog, QuoteLogError, QuoteServed};
use sqlx::SqlitePool;
use ulid::Ulid;

pub struct SqliteQuoteLog {
    pool: SqlitePool,
    clock: Arc<dyn Clock>,
}

impl SqliteQuoteLog {
    pub fn new(pool: SqlitePool, clock: Arc<dyn Clock>) -> Self {
        Self { pool, clock }
    }

    pub async fn migrate(&self) -> Result<(), QuoteLogError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(db)
    }
}

#[async_trait]
impl QuoteLog for SqliteQuoteLog {
    async fn record(&self, quote: &QuoteServed) -> Result<(), QuoteLogError> {
        let id = Ulid::new().to_string();
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query(
            "INSERT INTO quote_event (id, chain_id, pair_lo, pair_hi, served_at, latency_ms) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(quote.chain_id.0 as i64)
        .bind(quote.pair.lo.as_slice())
        .bind(quote.pair.hi.as_slice())
        .bind(self.clock.now_unix() as i64)
        .bind(quote.latency_ms as i64)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        for p in &quote.participants {
            sqlx::query(
                "INSERT OR IGNORE INTO quote_participant (quote_id, maker, strategy_hash) \
                 VALUES (?, ?, ?)",
            )
            .bind(&id)
            .bind(p.maker.0.as_slice())
            .bind(p.strategy_hash.0.as_slice())
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }
        tx.commit().await.map_err(db)
    }
}

fn db(e: impl std::fmt::Display) -> QuoteLogError {
    QuoteLogError::Db(e.to_string())
}
