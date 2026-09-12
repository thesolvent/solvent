//! The SQLite [`OrderLog`]: one row per order the feed showed us, verdict included.

use async_trait::async_trait;
use solvent_core::deps::order_log::{OrderLog, OrderLogError, OrderObserved};
use solvent_core::primitives::IntentId;
use sqlx::{Row, SqlitePool};

/// One order the feed delivered, as the explorer reads it back.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ObservedRow {
    pub order_hash: String,
    pub source: String,
    pub token_in: String,
    pub token_out: Option<String>,
    pub amount_in: String,
    pub required_out: Option<String>,
    pub verdict: String,
    pub reason: Option<String>,
    pub indicative_in: Option<String>,
    pub seen_at: i64,
}

pub struct SqliteOrderLog {
    pool: SqlitePool,
}

impl SqliteOrderLog {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// The newest orders first — the explorer's feed. Columns are read by name: a positional
    /// tuple would decode silently wrong if a later migration reorders them.
    pub async fn recent(&self, limit: i64) -> Result<Vec<ObservedRow>, OrderLogError> {
        let rows = sqlx::query(
            "SELECT order_hash, source, token_in, token_out, amount_in, required_out,
                    verdict, reason, indicative_in, seen_at
             FROM observed_order ORDER BY seen_at DESC, rowid DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter()
            .map(|row| {
                Ok(ObservedRow {
                    order_hash: hex(&row.try_get::<Vec<u8>, _>("order_hash").map_err(db)?),
                    source: row.try_get("source").map_err(db)?,
                    token_in: hex(&row.try_get::<Vec<u8>, _>("token_in").map_err(db)?),
                    token_out: row
                        .try_get::<Option<Vec<u8>>, _>("token_out")
                        .map_err(db)?
                        .as_deref()
                        .map(hex),
                    amount_in: row.try_get("amount_in").map_err(db)?,
                    required_out: row.try_get("required_out").map_err(db)?,
                    verdict: row.try_get("verdict").map_err(db)?,
                    reason: row.try_get("reason").map_err(db)?,
                    indicative_in: row.try_get("indicative_in").map_err(db)?,
                    seen_at: row.try_get("seen_at").map_err(db)?,
                })
            })
            .collect()
    }
}

#[async_trait]
impl OrderLog for SqliteOrderLog {
    async fn record(&self, order: &OrderObserved<'_>) -> Result<(), OrderLogError> {
        let intent = order.intent;
        let delivery = intent.delivery(order.seen_at);
        sqlx::query(
            "INSERT INTO observed_order
                 (order_hash, source, chain, token_in, token_out, amount_in, required_out,
                  verdict, reason, seen_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (order_hash) DO NOTHING",
        )
        .bind(intent.id.0.as_slice().to_vec())
        .bind(order.source.as_str())
        .bind(i64::try_from(intent.origin_chain.0).unwrap_or(i64::MAX))
        .bind(intent.input.token.as_slice().to_vec())
        .bind(delivery.map(|d| d.token.as_slice().to_vec()))
        .bind(intent.input.curve.amount_at(order.seen_at).to_string())
        .bind(delivery.map(|d| d.amount.to_string()))
        .bind(order.verdict.as_str())
        .bind(order.verdict.reason())
        .bind(i64::try_from(order.seen_at).unwrap_or(i64::MAX))
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn record_quote(
        &self,
        order: IntentId,
        indicative_in: &str,
    ) -> Result<(), OrderLogError> {
        sqlx::query("UPDATE observed_order SET indicative_in = ? WHERE order_hash = ?")
            .bind(indicative_in)
            .bind(order.0.as_slice().to_vec())
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> String {
    format!("0x{}", alloy::hex::encode(bytes))
}

fn db(e: sqlx::Error) -> OrderLogError {
    OrderLogError::Db(e.to_string())
}
