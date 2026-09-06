//! `GET /v1/stats` — the protocol stat tiles. `block_height` comes from the cached chain head;
//! `events_24h` from the event log's recorded times; `trades_settled`/`confirmed_pct`/
//! `median_impact_pct` from the trade store; `active_makers`/`quoting_now` from the live registry.
//! `median_impact_pct` is `null` until a settled trade carries an impact; `quoting_now` counts
//! priceable active strategies.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use serde::Serialize;
use solvent_core::primitives::registry::CurveSpec;
use solvent_core::primitives::ChainId;
use solvent_core::SolventError;

use crate::http::primitives::{ApiResult, Response};
use crate::http::state::AppState;

const DAY_SECS: u64 = 86_400;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Stats {
    pub block_height: u64,
    pub events_24h: Option<u64>,
    pub trades_settled: Option<u64>,
    pub confirmed_pct: Option<f64>,
    pub median_impact_pct: Option<f64>,
    pub active_makers: Option<u64>,
    pub quoting_now: Option<u64>,
}

#[utoipa::path(get, path = "/v1/stats", responses((status = 200, body = Response<Stats>)))]
pub async fn stats(State(state): State<AppState>) -> ApiResult<Stats> {
    let chain = ChainId(state.config.chain_id);
    let since = unix_now().saturating_sub(DAY_SECS);
    let events_24h = state
        .registry_store
        .count_since(chain, since)
        .await
        .map_err(SolventError::from)?;

    let trades = state.trades.stats().await.map_err(SolventError::from)?;
    // Success rate over on-chain outcomes only: confirmed vs failed. Declines are settled but
    // aren't fill attempts, so they stay out of the denominator.
    let attempted = trades.confirmed + trades.failed;
    let confirmed_pct = (attempted > 0).then(|| 100.0 * trades.confirmed as f64 / attempted as f64);

    // Distinct makers with a live strategy, and how many of those strategies can quote now.
    let snapshot = state.registry.load();
    let mut makers = BTreeSet::new();
    let mut quoting_now = 0u64;
    for strategy in snapshot.active_strategies() {
        makers.insert(strategy.key.maker);
        if matches!(strategy.curve, CurveSpec::Priceable { .. }) {
            quoting_now += 1;
        }
    }

    Ok(Response::ok(Stats {
        block_height: state.head.latest(),
        events_24h: Some(events_24h),
        trades_settled: Some(trades.settled),
        confirmed_pct,
        median_impact_pct: trades.median_impact_pct,
        active_makers: Some(makers.len() as u64),
        quoting_now: Some(quoting_now),
    }))
}

/// Unix seconds now; a clock before the epoch (impossible in practice) reads 0.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
