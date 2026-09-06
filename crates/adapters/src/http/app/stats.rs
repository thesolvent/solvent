//! `GET /v1/stats` — the protocol stat tiles. Only `block_height` is populated, read from the
//! cached chain head (not an RPC per request); the counts and rates have no data source yet, so
//! they serialize as `null`.

use axum::extract::State;
use serde::Serialize;

use crate::http::primitives::Response;
use crate::http::state::AppState;

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
