//! SQLite maker-metrics reads over `quote_events` + `trade`/`trade_leg`. Big-int volumes are summed
//! in Rust (SQLite `SUM` on a decimal-string column goes through `REAL` and loses precision), and the
//! p50 / day buckets are computed in Rust too. All figures are raw — USD/fees/APY are the DTO's job.

use std::{collections::BTreeMap, ops::Range};

use alloy::primitives::{Address, U256};
use async_trait::async_trait;
use solvent_core::deps::maker_metrics::{
    MakerMetrics, MakerMetricsError, MakerMetricsStore, PairMetrics, PositionMetrics, TokenVolume,
};
use solvent_core::primitives::maker::MakerActivityBucket;
use solvent_core::primitives::registry::TokenPair;
use solvent_core::primitives::{MakerId, StrategyHash};
use sqlx::SqlitePool;

pub struct SqliteMakerMetrics {
    pool: SqlitePool,
}

impl SqliteMakerMetrics {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn migrate(&self) -> Result<(), MakerMetricsError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(db)
    }

    /// Per-token flow for confirmed fills owned by `owner_col` (`l.maker` or `l.strategy_hash`),
    /// summed in Rust to keep U256 precision. `token_col`/`amount_col` pick the side: `token_out`/
    /// `amount_out` for delivered volume (outflow), `token_in`/`amount_in` for received (inflow).
    async fn flow(
        &self,
        token_col: &str,
        amount_col: &str,
        owner_col: &str,
        key: &[u8],
        window: Range<u64>,
    ) -> Result<Vec<TokenVolume>, MakerMetricsError> {
        let rows: Vec<(Vec<u8>, String)> = sqlx::query_as(&format!(
            "SELECT t.{token_col}, l.{amount_col} FROM trade t JOIN trade_leg l ON l.trade_id = t.id \
             WHERE t.status = 'confirmed' AND t.settled_at >= ? AND t.settled_at < ? AND l.{owner_col} = ?"
        ))
        .bind(i64::try_from(window.start).map_err(db)?)
        .bind(i64::try_from(window.end).map_err(db)?)
        .bind(key)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        sum_by_token(rows)
    }
}

/// Sum `(token_bytes, amount_str)` rows per token, keeping U256 precision.
fn sum_by_token(rows: Vec<(Vec<u8>, String)>) -> Result<Vec<TokenVolume>, MakerMetricsError> {
    let mut by_token: BTreeMap<Address, U256> = BTreeMap::new();
    for (token, amount) in rows {
        let token = Address::try_from(token.as_slice()).map_err(db)?;
        let amount = U256::from_str_radix(&amount, 10).map_err(db)?;
        let entry = by_token.entry(token).or_default();
        *entry = entry
            .checked_add(amount)
            .ok_or_else(|| db("token volume overflow"))?;
    }
    Ok(by_token
        .into_iter()
        .map(|(token, base_units)| TokenVolume { token, base_units })
        .collect())
}

#[async_trait]
impl MakerMetricsStore for SqliteMakerMetrics {
    async fn maker(
        &self,
        maker: MakerId,
        window: Range<u64>,
    ) -> Result<MakerMetrics, MakerMetricsError> {
        let key = maker.0;
        let fills: Vec<FillTime> = sqlx::query_as(
            "SELECT t.created_at, t.settled_at FROM trade t \
             WHERE t.status = 'confirmed' AND t.settled_at >= ? AND t.settled_at < ? \
               AND EXISTS (SELECT 1 FROM trade_leg l WHERE l.trade_id = t.id AND l.maker = ?)",
        )
        .bind(i64::try_from(window.start).map_err(db)?)
        .bind(i64::try_from(window.end).map_err(db)?)
        .bind(key.as_slice())
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        let quotes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM quote_event e WHERE e.served_at >= ? AND e.served_at < ? \
             AND EXISTS (SELECT 1 FROM quote_participant p WHERE p.quote_id = e.id AND p.maker = ?)",
        )
        .bind(i64::try_from(window.start).map_err(db)?)
        .bind(i64::try_from(window.end).map_err(db)?)
        .bind(key.as_slice())
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;

        Ok(MakerMetrics {
            fills: fills.len() as u64,
            activity: activity_buckets(&fills, window.clone()),
            last_fill_at: fills
                .iter()
                .filter_map(|f| u64::try_from(f.settled_at).ok())
                .max(),
            volume: self
                .flow(
                    "token_out",
                    "amount_out",
                    "maker",
                    key.as_slice(),
                    window.clone(),
                )
                .await?,
            inflow: self
                .flow("token_in", "amount_in", "maker", key.as_slice(), window)
                .await?,
            quotes: quotes as u64,
            latency_p50_ms: p50(fills.iter().filter_map(FillTime::latency_ms).collect()),
        })
    }

    async fn position(
        &self,
        strategy: StrategyHash,
        pair: TokenPair,
        window: Range<u64>,
    ) -> Result<PositionMetrics, MakerMetricsError> {
        let key = strategy.0;
        let fills: Vec<FillTime> = sqlx::query_as(
            "SELECT created_at, settled_at FROM trade t \
             WHERE t.status = 'confirmed' AND t.settled_at >= ? AND t.settled_at < ? \
               AND EXISTS (SELECT 1 FROM trade_leg l WHERE l.trade_id = t.id AND l.strategy_hash = ?)",
        )
        .bind(i64::try_from(window.start).map_err(db)?)
        .bind(i64::try_from(window.end).map_err(db)?)
        .bind(key.as_slice())
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        let participated: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM quote_participant p JOIN quote_event e ON e.id = p.quote_id \
             WHERE p.strategy_hash = ? AND e.served_at >= ? AND e.served_at < ?",
        )
        .bind(key.as_slice())
        .bind(i64::try_from(window.start).map_err(db)?)
        .bind(i64::try_from(window.end).map_err(db)?)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;

        let pair_quotes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM quote_event e \
             WHERE e.pair_lo = ? AND e.pair_hi = ? AND e.served_at >= ? AND e.served_at < ?",
        )
        .bind(pair.lo.as_slice())
        .bind(pair.hi.as_slice())
        .bind(i64::try_from(window.start).map_err(db)?)
        .bind(i64::try_from(window.end).map_err(db)?)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;

        let quote_uptime_pct =
            (pair_quotes > 0).then(|| participated as f64 / pair_quotes as f64 * 100.0);

        Ok(PositionMetrics {
            fills: fills.len() as u64,
            daily_fills: activity_buckets(&fills, window.clone())
                .into_iter()
                .map(|b| b.fills)
                .collect(),
            volume: self
                .flow(
                    "token_out",
                    "amount_out",
                    "strategy_hash",
                    key.as_slice(),
                    window,
                )
                .await?,
            last_fill_at: fills
                .iter()
                .filter_map(|f| u64::try_from(f.settled_at).ok())
                .max(),
            quote_uptime_pct,
        })
    }

    async fn pair_fills(
        &self,
        pairs: &[TokenPair],
        window: Range<u64>,
    ) -> Result<u64, MakerMetricsError> {
        if pairs.is_empty() {
            return Ok(0);
        }
        // A trade's pair is unordered; match either orientation of each maker pair.
        let clause = pairs
            .iter()
            .map(|_| "(token_in = ? AND token_out = ?) OR (token_in = ? AND token_out = ?)")
            .collect::<Vec<_>>()
            .join(" OR ");
        let sql = format!(
            "SELECT COUNT(*) FROM trade WHERE status = 'confirmed' AND settled_at >= ? AND settled_at < ? AND ({clause})"
        );
        let mut q = sqlx::query_scalar(&sql)
            .bind(i64::try_from(window.start).map_err(db)?)
            .bind(i64::try_from(window.end).map_err(db)?);
        for pair in pairs {
            q = q
                .bind(pair.lo.as_slice().to_vec())
                .bind(pair.hi.as_slice().to_vec())
                .bind(pair.hi.as_slice().to_vec())
                .bind(pair.lo.as_slice().to_vec());
        }
        let count: i64 = q.fetch_one(&self.pool).await.map_err(db)?;
        Ok(count as u64)
    }

    async fn pair_activity(
        &self,
        pair: TokenPair,
        since: u64,
    ) -> Result<PairMetrics, MakerMetricsError> {
        // A trade's pair is unordered, so match either orientation.
        const ORIENTATION: &str =
            "((token_in = ? AND token_out = ?) OR (token_in = ? AND token_out = ?))";
        let (lo, hi) = (pair.lo.as_slice().to_vec(), pair.hi.as_slice().to_vec());
        let since = since as i64;

        let fills: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM trade \
             WHERE status = 'confirmed' AND settled_at >= ? AND {ORIENTATION}"
        ))
        .bind(since)
        .bind(&lo)
        .bind(&hi)
        .bind(&hi)
        .bind(&lo)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;

        // Joining the legs multiplies rows per trade, which is what the volume sum wants; the fill
        // count above deliberately counts trades instead.
        let rows: Vec<(Vec<u8>, String)> = sqlx::query_as(&format!(
            "SELECT t.token_out, l.amount_out FROM trade t JOIN trade_leg l ON l.trade_id = t.id \
             WHERE t.status = 'confirmed' AND t.settled_at >= ? AND {ORIENTATION}"
        ))
        .bind(since)
        .bind(&lo)
        .bind(&hi)
        .bind(&hi)
        .bind(&lo)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        Ok(PairMetrics {
            fills: fills as u64,
            volume: sum_by_token(rows)?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct FillTime {
    created_at: i64,
    settled_at: i64,
}

impl FillTime {
    fn latency_ms(&self) -> Option<u64> {
        u64::try_from(self.settled_at.checked_sub(self.created_at)?)
            .ok()?
            .checked_mul(1000)
    }
}

fn activity_buckets(fills: &[FillTime], window: Range<u64>) -> Vec<MakerActivityBucket> {
    let duration = window.end.saturating_sub(window.start);
    (0..7)
        .map(|i| {
            let from = window.start + duration * i / 7;
            let to = window.start + duration * (i + 1) / 7;
            let mut bucket = MakerActivityBucket::empty(from, to);
            let mut latencies = Vec::new();
            for fill in fills.iter().filter(|fill| {
                u64::try_from(fill.settled_at).is_ok_and(|at| (from..to).contains(&at))
            }) {
                bucket.fills += 1;
                if let Some(latency) = fill.latency_ms() {
                    latencies.push(latency);
                }
            }
            bucket.latency_p50_ms = p50(latencies);
            bucket
        })
        .collect()
}

/// Lower median; an empty sample has no latency, rather than zero latency.
fn p50(mut values: Vec<u64>) -> Option<u64> {
    let middle = values.len().checked_sub(1)? / 2;
    let (_, median, _) = values.select_nth_unstable(middle);
    Some(*median)
}

fn db(e: impl std::fmt::Display) -> MakerMetricsError {
    MakerMetricsError::Db(e.to_string())
}
