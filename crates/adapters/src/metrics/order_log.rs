//! The SQLite [`OrderLog`]: one row per order the feed showed us, verdict included.

use async_trait::async_trait;
use solvent_core::deps::order_log::{OrderLog, OrderLogError, OrderObserved};
use solvent_core::primitives::IntentId;
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};

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
    /// The trade this order became, if it was ever attempted — `None` when it was refused at the
    /// door or declined as unprofitable before routing ever reserved anything.
    pub trade_status: Option<String>,
    /// The trade's own id (a ULID) — what a reader clicks through to for the trade's full detail.
    pub trade_id: Option<String>,
    pub trade_tx_hash: Option<String>,
    /// Why the trade declined — the admission rule, the sim gate's real on-chain revert reason, or
    /// the margin call. Only present once the order became a trade.
    pub trade_decline_reason: Option<String>,
}

/// Which derived order-feed state to filter to — the same bucketing the explorer UI computes from
/// verdict plus trade lifecycle, so a filtered page count matches what the UI actually shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStateFilter {
    Refused,
    Filled,
    Declined,
    FailedOnChain,
    Unprofitable,
    Pricing,
}

impl OrderStateFilter {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "refused" => Some(Self::Refused),
            "filled" => Some(Self::Filled),
            "declined" => Some(Self::Declined),
            "failed_onchain" => Some(Self::FailedOnChain),
            "unprofitable" => Some(Self::Unprofitable),
            "pricing" => Some(Self::Pricing),
            _ => None,
        }
    }

    fn sql(self) -> &'static str {
        match self {
            Self::Refused => "observed_order.verdict = 'dropped'",
            Self::Filled => {
                "observed_order.verdict = 'admitted' AND trade.status IN ('confirmed', 'submitted')"
            }
            Self::FailedOnChain => {
                "observed_order.verdict = 'admitted' AND trade.status = 'failed'"
            }
            Self::Declined => "observed_order.verdict = 'admitted' AND trade.status = 'declined'",
            Self::Unprofitable => {
                "observed_order.verdict = 'admitted' AND trade.status IS NULL \
                 AND observed_order.indicative_in IS NOT NULL"
            }
            Self::Pricing => {
                "observed_order.verdict = 'admitted' AND trade.status IS NULL \
                 AND observed_order.indicative_in IS NULL"
            }
        }
    }
}

/// The order feed's filterable columns — the same facets the explorer UI lets a reader narrow by.
#[derive(Debug, Default, Clone)]
pub struct OrderFeedFilter {
    pub source: Option<String>,
    pub token_in: Option<Vec<u8>>,
    pub token_out: Option<Vec<u8>>,
    pub state: Option<OrderStateFilter>,
}

pub struct SqliteOrderLog {
    pool: SqlitePool,
}

impl SqliteOrderLog {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// The newest orders first, paged and filtered. Columns are read by name: a positional tuple
    /// would decode silently wrong if a later migration reorders them.
    pub async fn recent(
        &self,
        limit: i64,
        offset: i64,
        filter: &OrderFeedFilter,
    ) -> Result<Vec<ObservedRow>, OrderLogError> {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT observed_order.order_hash, observed_order.source, observed_order.token_in,
                    observed_order.token_out, observed_order.amount_in, observed_order.required_out,
                    observed_order.verdict, observed_order.reason, observed_order.indicative_in,
                    observed_order.seen_at, trade.status AS trade_status, trade.id AS trade_id,
                    trade.tx_hash AS trade_tx_hash, trade.decline_reason AS trade_decline_reason
             FROM observed_order
             LEFT JOIN trade ON trade.order_hash = observed_order.order_hash
             WHERE 1 = 1",
        );
        push_filter(&mut query, filter);
        query
            .push(" ORDER BY observed_order.seen_at DESC, observed_order.rowid DESC LIMIT ")
            .push_bind(limit)
            .push(" OFFSET ")
            .push_bind(offset);
        let rows = query.build().fetch_all(&self.pool).await.map_err(db)?;
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
                    trade_status: row.try_get("trade_status").map_err(db)?,
                    trade_id: row.try_get("trade_id").map_err(db)?,
                    trade_tx_hash: row
                        .try_get::<Option<Vec<u8>>, _>("trade_tx_hash")
                        .map_err(db)?
                        .as_deref()
                        .map(hex),
                    trade_decline_reason: row.try_get("trade_decline_reason").map_err(db)?,
                })
            })
            .collect()
    }

    /// How many orders match `filter` — the denominator `recent`'s filtered page sits inside.
    pub async fn count(&self, filter: &OrderFeedFilter) -> Result<i64, OrderLogError> {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT COUNT(*) FROM observed_order
             LEFT JOIN trade ON trade.order_hash = observed_order.order_hash
             WHERE 1 = 1",
        );
        push_filter(&mut query, filter);
        query
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(db)
    }
}

fn push_filter(query: &mut QueryBuilder<'_, Sqlite>, filter: &OrderFeedFilter) {
    if let Some(source) = &filter.source {
        query
            .push(" AND observed_order.source = ")
            .push_bind(source.clone());
    }
    if let Some(token_in) = &filter.token_in {
        query
            .push(" AND observed_order.token_in = ")
            .push_bind(token_in.clone());
    }
    if let Some(token_out) = &filter.token_out {
        query
            .push(" AND observed_order.token_out = ")
            .push_bind(token_out.clone());
    }
    if let Some(state) = filter.state {
        query.push(" AND ").push(state.sql());
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
