//! The SQLite trade store (sqlx) behind the `TradeStore` port. `create` is one transaction that
//! dedups on the order hash; `advance`/`settle` are guarded so a replayed transition is a no-op.

use alloy::primitives::{Address, Bytes, B256, U256};
use async_trait::async_trait;
use solvent_core::primitives::ingest::OrderSource;
use solvent_core::{
    deps::trade::{
        CreateResult, MakerFill, Page, Settlement, TradeFilter, TradeStats, TradeStore,
        TradeStoreError,
    },
    primitives::{
        trade::{Trade, TradeAttempt, TradeId, TradeInfo, TradeLeg, TradeStatus},
        IntentId, MakerId, StrategyHash,
    },
};
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool, Transaction};

pub struct SqliteTradeStore {
    pool: SqlitePool,
}

impl SqliteTradeStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Apply the embedded schema migrations.
    pub async fn migrate(&self) -> Result<(), TradeStoreError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(db)
    }
}

fn db(e: impl std::fmt::Display) -> TradeStoreError {
    TradeStoreError::Db(e.to_string())
}

/// The median of `values`, or `None` if empty. Sorts in place (NaN-safe via `total_cmp`).
fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    })
}

// ---- encoding: domain -> column --------------------------------------------

fn i64_of(value: u64) -> Result<i64, TradeStoreError> {
    i64::try_from(value).map_err(|_| db(format!("value {value} exceeds i64")))
}

fn amount_text(value: &U256) -> String {
    value.to_string()
}

fn bytes_of(value: impl AsRef<[u8]>) -> Vec<u8> {
    value.as_ref().to_vec()
}

// ---- decoding: column -> domain --------------------------------------------

fn amount(row: &SqliteRow, col: &str) -> Result<U256, TradeStoreError> {
    let text: String = row.try_get(col).map_err(db)?;
    U256::from_str_radix(&text, 10)
        .map_err(|_| db(format!("column '{col}': invalid u256 '{text}'")))
}

fn opt_amount(row: &SqliteRow, col: &str) -> Result<Option<U256>, TradeStoreError> {
    match row.try_get::<Option<String>, _>(col).map_err(db)? {
        Some(text) => U256::from_str_radix(&text, 10)
            .map(Some)
            .map_err(|_| db(format!("column '{col}': invalid u256 '{text}'"))),
        None => Ok(None),
    }
}

fn address(row: &SqliteRow, col: &str) -> Result<Address, TradeStoreError> {
    let bytes: Vec<u8> = row.try_get(col).map_err(db)?;
    <[u8; 20]>::try_from(bytes.as_slice())
        .map(Address::from)
        .map_err(|_| {
            db(format!(
                "column '{col}': expected 20 bytes, got {}",
                bytes.len()
            ))
        })
}

fn hash(row: &SqliteRow, col: &str) -> Result<B256, TradeStoreError> {
    decode_hash(col, row.try_get(col).map_err(db)?)
}

fn opt_hash(row: &SqliteRow, col: &str) -> Result<Option<B256>, TradeStoreError> {
    row.try_get::<Option<Vec<u8>>, _>(col)
        .map_err(db)?
        .map(|bytes| decode_hash(col, bytes))
        .transpose()
}

fn decode_hash(col: &str, bytes: Vec<u8>) -> Result<B256, TradeStoreError> {
    B256::try_from(bytes.as_slice()).map_err(|_| {
        db(format!(
            "column '{col}': expected 32 bytes, got {}",
            bytes.len()
        ))
    })
}

fn count(row: &SqliteRow, col: &str) -> Result<u64, TradeStoreError> {
    let value: i64 = row.try_get(col).map_err(db)?;
    u64::try_from(value).map_err(|_| db(format!("column '{col}': negative value {value}")))
}

fn opt_count(row: &SqliteRow, col: &str) -> Result<Option<u64>, TradeStoreError> {
    row.try_get::<Option<i64>, _>(col)
        .map_err(db)?
        .map(|value| {
            u64::try_from(value).map_err(|_| db(format!("column '{col}': negative value {value}")))
        })
        .transpose()
}

fn status(row: &SqliteRow, col: &str) -> Result<TradeStatus, TradeStoreError> {
    row.try_get::<String, _>(col)
        .map_err(db)?
        .parse()
        .map_err(db)
}

fn row_to_trade(row: &SqliteRow) -> Result<Trade, TradeStoreError> {
    Ok(Trade {
        id: row
            .try_get::<String, _>("id")
            .map_err(db)?
            .parse()
            .map_err(db)?,
        order_hash: IntentId(hash(row, "order_hash")?),
        taker: address(row, "taker")?,
        token_in: address(row, "token_in")?,
        token_out: address(row, "token_out")?,
        amount_in: amount(row, "amount_in")?,
        min_amount_out: amount(row, "min_amount_out")?,
        amount_out: opt_amount(row, "amount_out")?,
        status: status(row, "status")?,
        deadline_block: count(row, "deadline_block")?,
        signature: row
            .try_get::<Option<Vec<u8>>, _>("signature")
            .map_err(db)?
            .map(Bytes::from),
        price_impact_pct: row.try_get("price_impact_pct").map_err(db)?,
        surplus: opt_amount(row, "surplus")?,
        tx_hash: opt_hash(row, "tx_hash")?,
        block_number: opt_count(row, "block_number")?,
        created_at: count(row, "created_at")?,
        settled_at: opt_count(row, "settled_at")?,
        indicative_amount_in: opt_amount(row, "indicative_amount_in")?,
        source: match row.try_get::<String, _>("source").map_err(db)?.as_str() {
            "uniswapx" => OrderSource::UniswapX,
            _ => OrderSource::Solvent,
        },
        token_in_price_usd: row.try_get("token_in_price_usd").map_err(db)?,
        token_out_price_usd: row.try_get("token_out_price_usd").map_err(db)?,
    })
}

fn row_to_attempt(row: &SqliteRow) -> Result<TradeAttempt, TradeStoreError> {
    Ok(TradeAttempt {
        status: status(row, "status")?,
        at: count(row, "at")?,
    })
}

fn row_to_leg(row: &SqliteRow) -> Result<TradeLeg, TradeStoreError> {
    Ok(TradeLeg {
        maker: MakerId(address(row, "maker")?),
        strategy_hash: StrategyHash(hash(row, "strategy_hash")?),
        amount_in: amount(row, "amount_in")?,
        amount_out: amount(row, "amount_out")?,
    })
}

// ---- write statements (each runs inside the caller's transaction) -----------

/// Insert the trade header, deduping on the order hash. `true` if newly inserted; `false` if a
/// trade for the same order already existed.
async fn insert_trade(
    tx: &mut Transaction<'_, Sqlite>,
    trade: &Trade,
) -> Result<bool, TradeStoreError> {
    let affected = sqlx::query(
        "INSERT INTO trade (
             id, order_hash, taker, token_in, token_out, amount_in, min_amount_out, amount_out,
             status, status_rank, deadline_block, signature, price_impact_pct, surplus, tx_hash,
             block_number, created_at, settled_at, token_in_price_usd, token_out_price_usd,
             indicative_amount_in, source)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (order_hash) DO NOTHING",
    )
    .bind(trade.id.to_string())
    .bind(bytes_of(trade.order_hash.0))
    .bind(bytes_of(trade.taker))
    .bind(bytes_of(trade.token_in))
    .bind(bytes_of(trade.token_out))
    .bind(amount_text(&trade.amount_in))
    .bind(amount_text(&trade.min_amount_out))
    .bind(trade.amount_out.as_ref().map(amount_text))
    .bind(trade.status.as_str())
    .bind(i64::from(trade.status.rank()))
    .bind(i64_of(trade.deadline_block)?)
    .bind(trade.signature.as_ref().map(bytes_of))
    .bind(trade.price_impact_pct)
    .bind(trade.surplus.as_ref().map(amount_text))
    .bind(trade.tx_hash.map(bytes_of))
    .bind(trade.block_number.map(i64_of).transpose()?)
    .bind(i64_of(trade.created_at)?)
    .bind(trade.settled_at.map(i64_of).transpose()?)
    .bind(trade.token_in_price_usd)
    .bind(trade.token_out_price_usd)
    .bind(trade.indicative_amount_in.as_ref().map(amount_text))
    .bind(trade.source.as_str())
    .execute(&mut **tx)
    .await
    .map_err(db)?
    .rows_affected();
    Ok(affected == 1)
}

/// Insert one routed leg at position `idx`.
async fn insert_leg(
    tx: &mut Transaction<'_, Sqlite>,
    trade_id: &TradeId,
    idx: usize,
    leg: &TradeLeg,
) -> Result<(), TradeStoreError> {
    sqlx::query(
        "INSERT INTO trade_leg (trade_id, idx, maker, strategy_hash, amount_in, amount_out)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(trade_id.to_string())
    .bind(i64::try_from(idx).map_err(db)?)
    .bind(bytes_of(leg.maker.0))
    .bind(bytes_of(leg.strategy_hash.0))
    .bind(amount_text(&leg.amount_in))
    .bind(amount_text(&leg.amount_out))
    .execute(&mut **tx)
    .await
    .map_err(db)?;
    Ok(())
}

/// Append one lifecycle stage; a stage already recorded for the trade is left as-is.
async fn insert_attempt(
    tx: &mut Transaction<'_, Sqlite>,
    trade_id: &TradeId,
    status: TradeStatus,
    at: u64,
) -> Result<(), TradeStoreError> {
    sqlx::query(
        "INSERT INTO trade_attempt (trade_id, status, at) VALUES (?, ?, ?)
         ON CONFLICT DO NOTHING",
    )
    .bind(trade_id.to_string())
    .bind(status.as_str())
    .bind(i64_of(at)?)
    .execute(&mut **tx)
    .await
    .map_err(db)?;
    Ok(())
}

/// The id of the trade already recorded for `order_hash` — the dedup target of a conflicting insert.
async fn existing_trade_id(
    tx: &mut Transaction<'_, Sqlite>,
    order_hash: IntentId,
) -> Result<TradeId, TradeStoreError> {
    let id: String = sqlx::query_scalar("SELECT id FROM trade WHERE order_hash = ?")
        .bind(bytes_of(order_hash.0))
        .fetch_one(&mut **tx)
        .await
        .map_err(db)?;
    id.parse().map_err(db)
}

#[async_trait]
impl TradeStore for SqliteTradeStore {
    async fn create(
        &self,
        trade: &Trade,
        legs: &[TradeLeg],
        attempts: &[TradeAttempt],
    ) -> Result<CreateResult, TradeStoreError> {
        let mut tx = self.pool.begin().await.map_err(db)?;

        let created = insert_trade(&mut tx, trade).await?;
        let id = if created {
            for (idx, leg) in legs.iter().enumerate() {
                insert_leg(&mut tx, &trade.id, idx, leg).await?;
            }
            for attempt in attempts {
                insert_attempt(&mut tx, &trade.id, attempt.status, attempt.at).await?;
            }
            trade.id
        } else {
            existing_trade_id(&mut tx, trade.order_hash).await?
        };

        tx.commit().await.map_err(db)?;
        Ok(CreateResult { id, created })
    }

    async fn advance(
        &self,
        id: &TradeId,
        status: TradeStatus,
        at: u64,
    ) -> Result<(), TradeStoreError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query(
            "UPDATE trade SET status = ?, status_rank = ?
             WHERE id = ? AND status_rank < ? AND settled_at IS NULL",
        )
        .bind(status.as_str())
        .bind(i64::from(status.rank()))
        .bind(id.to_string())
        .bind(i64::from(status.rank()))
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        insert_attempt(&mut tx, id, status, at).await?;
        tx.commit().await.map_err(db)?;
        Ok(())
    }

    async fn settle(&self, id: &TradeId, outcome: &Settlement) -> Result<(), TradeStoreError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query(
            "UPDATE trade
             SET status = ?, status_rank = ?, amount_out = ?, tx_hash = ?, block_number = ?,
                 settled_at = ?
             WHERE id = ? AND settled_at IS NULL",
        )
        .bind(outcome.status.as_str())
        .bind(i64::from(outcome.status.rank()))
        .bind(outcome.amount_out.as_ref().map(amount_text))
        .bind(outcome.tx_hash.map(bytes_of))
        .bind(outcome.block_number.map(i64_of).transpose()?)
        .bind(i64_of(outcome.at)?)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        insert_attempt(&mut tx, id, outcome.status, outcome.at).await?;
        tx.commit().await.map_err(db)?;
        Ok(())
    }

    async fn find_by_order(&self, order_hash: &IntentId) -> Result<Option<Trade>, TradeStoreError> {
        sqlx::query("SELECT * FROM trade WHERE order_hash = ?")
            .bind(bytes_of(order_hash.0))
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?
            .as_ref()
            .map(row_to_trade)
            .transpose()
    }

    async fn info(&self, id: &TradeId) -> Result<Option<TradeInfo>, TradeStoreError> {
        let Some(row) = sqlx::query("SELECT * FROM trade WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?
        else {
            return Ok(None);
        };

        let attempts = sqlx::query(
            "SELECT status, at FROM trade_attempt WHERE trade_id = ? ORDER BY at, status",
        )
        .bind(id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(db)?
        .iter()
        .map(row_to_attempt)
        .collect::<Result<_, _>>()?;

        let legs = sqlx::query("SELECT * FROM trade_leg WHERE trade_id = ? ORDER BY idx")
            .bind(id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .iter()
            .map(row_to_leg)
            .collect::<Result<_, _>>()?;

        Ok(Some(TradeInfo {
            trade: row_to_trade(&row)?,
            attempts,
            legs,
        }))
    }

    async fn stats(&self) -> Result<TradeStats, TradeStoreError> {
        let (settled, confirmed, failed): (i64, i64, i64) = sqlx::query_as(
            "SELECT
                 COUNT(*) FILTER (WHERE settled_at IS NOT NULL),
                 COUNT(*) FILTER (WHERE status = 'confirmed'),
                 COUNT(*) FILTER (WHERE status = 'failed')
             FROM trade",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;

        // Median in Rust over the settled trades that carry a price impact.
        let mut impacts: Vec<f64> = sqlx::query_scalar(
            "SELECT price_impact_pct FROM trade
             WHERE settled_at IS NOT NULL AND price_impact_pct IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        Ok(TradeStats {
            settled: u64::try_from(settled).map_err(|_| db("negative settled count"))?,
            confirmed: u64::try_from(confirmed).map_err(|_| db("negative confirmed count"))?,
            failed: u64::try_from(failed).map_err(|_| db("negative failed count"))?,
            median_impact_pct: median(&mut impacts),
        })
    }

    async fn list(&self, filter: &TradeFilter, page: &Page) -> Result<Vec<Trade>, TradeStoreError> {
        let mut query = QueryBuilder::<Sqlite>::new("SELECT * FROM trade WHERE 1 = 1");
        if let Some(status) = filter.status {
            query.push(" AND status = ").push_bind(status.as_str());
        }
        if let Some(taker) = filter.taker {
            query.push(" AND taker = ").push_bind(bytes_of(taker));
        }
        if let Some((a, b)) = filter.pair {
            let (a, b) = (bytes_of(a), bytes_of(b));
            query
                .push(" AND ((token_in = ")
                .push_bind(a.clone())
                .push(" AND token_out = ")
                .push_bind(b.clone())
                .push(") OR (token_in = ")
                .push_bind(b)
                .push(" AND token_out = ")
                .push_bind(a)
                .push("))");
        }
        if let Some(strategy_hash) = filter.strategy_hash {
            query
                .push(" AND EXISTS (SELECT 1 FROM trade_leg WHERE trade_leg.trade_id = trade.id AND strategy_hash = ")
                .push_bind(bytes_of(strategy_hash.0))
                .push(")");
        }
        if let Some(cursor) = &page.cursor {
            query.push(" AND id < ").push_bind(cursor.to_string());
        }
        query
            .push(" ORDER BY id DESC LIMIT ")
            .push_bind(i64::from(page.limit));

        query
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .iter()
            .map(row_to_trade)
            .collect()
    }

    async fn list_for_maker(
        &self,
        maker: Address,
        page: &Page,
        window: Option<std::ops::Range<u64>>,
    ) -> Result<Vec<MakerFill>, TradeStoreError> {
        // Select trade headers before joining their legs so the page limit cannot split a fill.
        let mut query = QueryBuilder::<Sqlite>::new(
            "WITH page AS (SELECT * FROM trade WHERE EXISTS \
             (SELECT 1 FROM trade_leg WHERE trade_leg.trade_id = trade.id AND maker = ",
        );
        query.push_bind(bytes_of(maker)).push(")");
        if let Some(window) = window {
            query
                .push(" AND status = 'confirmed' AND settled_at >= ")
                .push_bind(i64::try_from(window.start).map_err(db)?)
                .push(" AND settled_at < ")
                .push_bind(i64::try_from(window.end).map_err(db)?);
        }
        if let Some(cursor) = &page.cursor {
            query.push(" AND trade.id < ").push_bind(cursor.to_string());
        }
        query
            .push(" ORDER BY trade.id DESC LIMIT ")
            .push_bind(i64::from(page.limit))
            .push(
                ") SELECT page.*, \
                leg.amount_in AS leg_amount_in, leg.amount_out AS leg_amount_out \
                FROM page JOIN trade_leg leg ON leg.trade_id = page.id WHERE leg.maker = ",
            )
            .push_bind(bytes_of(maker))
            .push(" ORDER BY page.id DESC, leg.idx");

        let rows = query.build().fetch_all(&self.pool).await.map_err(db)?;
        let mut fills: Vec<MakerFill> = Vec::new();
        for row in &rows {
            let id: String = row.try_get("id").map_err(db)?;
            let amount_in = amount(row, "leg_amount_in")?;
            let amount_out = amount(row, "leg_amount_out")?;
            match fills.last_mut() {
                Some(fill) if fill.trade.id.to_string() == id => {
                    // SQLite numeric casts lose precision for U256 token amounts.
                    fill.amount_in = fill
                        .amount_in
                        .checked_add(amount_in)
                        .ok_or_else(|| db("maker input amount exceeds U256"))?;
                    fill.amount_out = fill
                        .amount_out
                        .checked_add(amount_out)
                        .ok_or_else(|| db("maker output amount exceeds U256"))?;
                }
                _ => fills.push(MakerFill::new(row_to_trade(row)?, amount_in, amount_out)),
            }
        }
        Ok(fills)
    }
}
