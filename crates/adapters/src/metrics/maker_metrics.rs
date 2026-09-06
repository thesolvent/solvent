//! SQLite maker-metrics reads over `quote_events` + `trade`/`trade_leg`. Big-int volumes are summed
//! in Rust (SQLite `SUM` on a decimal-string column goes through `REAL` and loses precision), and the
//! p50 / day buckets are computed in Rust too. All figures are raw — USD/fees/APY are the DTO's job.

use std::collections::BTreeMap;

use alloy::primitives::{Address, U256};
use async_trait::async_trait;
use solvent_core::deps::maker_metrics::{
    MakerMetrics, MakerMetricsError, MakerMetricsStore, PositionMetrics, TokenVolume,
};
use solvent_core::primitives::registry::TokenPair;
use solvent_core::primitives::{MakerId, StrategyHash};
use sqlx::SqlitePool;

const DAY: u64 = 86_400;

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

    /// Per-token delivered volume for confirmed fills matching `owner_clause` (`l.maker` or
    /// `l.strategy_hash`), summed in Rust to keep U256 precision.
    async fn volume(
        &self,
        column: &str,
        key: &[u8],
        since: i64,
    ) -> Result<Vec<TokenVolume>, MakerMetricsError> {
        let rows: Vec<(Vec<u8>, String)> = sqlx::query_as(&format!(
            "SELECT t.token_out, l.amount_out FROM trade t JOIN trade_leg l ON l.trade_id = t.id \
             WHERE t.status = 'confirmed' AND t.settled_at >= ? AND l.{column} = ?"
        ))
        .bind(since)
        .bind(key)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        let mut by_token: BTreeMap<Address, U256> = BTreeMap::new();
        for (token, amount) in rows {
            let token = Address::from_slice(&token);
            let amount = U256::from_str_radix(&amount, 10).unwrap_or(U256::ZERO);
            let entry = by_token.entry(token).or_default();
            *entry = entry.saturating_add(amount);
        }
        Ok(by_token
            .into_iter()
            .map(|(token, base_units)| TokenVolume { token, base_units })
            .collect())
    }
}

#[async_trait]
impl MakerMetricsStore for SqliteMakerMetrics {
    async fn maker(
        &self,
        maker: MakerId,
        since: u64,
        now: u64,
    ) -> Result<MakerMetrics, MakerMetricsError> {
        let key = maker.0;
        let (fills, last_fill_at): (i64, Option<i64>) = sqlx::query_as(
            "SELECT COUNT(*), MAX(settled_at) FROM trade t \
             WHERE t.status = 'confirmed' AND t.settled_at >= ? \
               AND EXISTS (SELECT 1 FROM trade_leg l WHERE l.trade_id = t.id AND l.maker = ?)",
        )
        .bind(since as i64)
        .bind(key.as_slice())
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;

        let seven_days_ago = now.saturating_sub(7 * DAY) as i64;
        let fill_times: Vec<i64> = sqlx::query_scalar(
            "SELECT settled_at FROM trade t \
             WHERE t.status = 'confirmed' AND t.settled_at >= ? \
               AND EXISTS (SELECT 1 FROM trade_leg l WHERE l.trade_id = t.id AND l.maker = ?)",
        )
        .bind(seven_days_ago)
        .bind(key.as_slice())
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        let quotes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM quote_participant p JOIN quote_event e ON e.id = p.quote_id \
             WHERE p.maker = ? AND e.served_at >= ?",
        )
        .bind(key.as_slice())
        .bind(since as i64)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;

        let latencies: Vec<i64> = sqlx::query_scalar(
            "SELECT e.latency_ms FROM quote_participant p JOIN quote_event e ON e.id = p.quote_id \
             WHERE p.maker = ? AND e.served_at >= ?",
        )
        .bind(key.as_slice())
        .bind(since as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        Ok(MakerMetrics {
            fills: fills as u64,
            fills_by_day: day_buckets(&fill_times, now),
            last_fill_at: last_fill_at.map(|t| t as u64),
            volume: self.volume("maker", key.as_slice(), since as i64).await?,
            quotes: quotes as u64,
            latency_p50_ms: p50(latencies),
        })
    }

    async fn position(
        &self,
        strategy: StrategyHash,
        pair: TokenPair,
        since: u64,
    ) -> Result<PositionMetrics, MakerMetricsError> {
        let key = strategy.0;
        let (fills, last_fill_at): (i64, Option<i64>) = sqlx::query_as(
            "SELECT COUNT(*), MAX(settled_at) FROM trade t \
             WHERE t.status = 'confirmed' AND t.settled_at >= ? \
               AND EXISTS (SELECT 1 FROM trade_leg l WHERE l.trade_id = t.id AND l.strategy_hash = ?)",
        )
        .bind(since as i64)
        .bind(key.as_slice())
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;

        let participated: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM quote_participant p JOIN quote_event e ON e.id = p.quote_id \
             WHERE p.strategy_hash = ? AND e.served_at >= ?",
        )
        .bind(key.as_slice())
        .bind(since as i64)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;

        let pair_quotes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM quote_event e \
             WHERE e.pair_lo = ? AND e.pair_hi = ? AND e.served_at >= ?",
        )
        .bind(pair.lo.as_slice())
        .bind(pair.hi.as_slice())
        .bind(since as i64)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;

        let quote_uptime_pct =
            (pair_quotes > 0).then(|| participated as f64 / pair_quotes as f64 * 100.0);

        Ok(PositionMetrics {
            fills: fills as u64,
            volume: self
                .volume("strategy_hash", key.as_slice(), since as i64)
                .await?,
            last_fill_at: last_fill_at.map(|t| t as u64),
            quote_uptime_pct,
        })
    }
}

/// Fills into 7 daily buckets ending at `now`, oldest first (`[0]` = 6 days ago, `[6]` = today).
fn day_buckets(times: &[i64], now: u64) -> [u64; 7] {
    let mut buckets = [0u64; 7];
    for &t in times {
        let Ok(t) = u64::try_from(t) else { continue };
        // A future timestamp saturates to day_ago 0 → today's bucket.
        let day_ago = (now.saturating_sub(t) / DAY) as usize;
        if day_ago < 7 {
            buckets[6 - day_ago] += 1;
        }
    }
    buckets
}

/// Lower-median (p50). `None` for an empty sample.
fn p50(mut values: Vec<i64>) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    u64::try_from(values[values.len() / 2]).ok()
}

fn db(e: impl std::fmt::Display) -> MakerMetricsError {
    MakerMetricsError::Db(e.to_string())
}
