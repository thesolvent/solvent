//! `GET /v1/stats` — the protocol stat tiles. Only `block_height` is live in M0, read from the
//! cached chain head (not an RPC per request); the counts and rates arrive with the trade store
//! (M2) and pricing (M3), so they serialize as `null` until then.

use axum::extract::State;
use serde::Serialize;

use crate::http::primitives::Response;
use crate::http::state::AppState;

#[derive(Debug, Serialize)]
pub struct Stats {
    pub block_height: u64,
    pub events_24h: Option<u64>,
    pub trades_settled: Option<u64>,
    pub confirmed_pct: Option<f64>,
    pub median_impact_pct: Option<f64>,
    pub active_makers: Option<u64>,
    pub quoting_now: Option<u64>,
}

pub async fn stats(State(state): State<AppState>) -> Response<Stats> {
    Response::ok(Stats {
        block_height: state.head.latest(),
        events_24h: None,
        trades_settled: None,
        confirmed_pct: None,
        median_impact_pct: None,
        active_makers: None,
        quoting_now: None,
    })
}
