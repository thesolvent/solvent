//! `GET /v1/makers` — the active-maker roster. `GET /v1/makers/{maker}` — one maker's dashboard.
//! `GET /v1/makers/{maker}/positions` — a maker's positions (list projection). `GET
//! /v1/positions/{hash}` — one position's full detail.

use alloy::primitives::{Address, B256};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use solvent_core::primitives::maker::{MakerDashboard, MakerSummary, Position};
use solvent_core::primitives::{MakerId, StrategyHash};
use solvent_core::SolventError;

use crate::http::dto::List;
use crate::http::primitives::{ApiResult, Response};
use crate::http::state::AppState;

/// The roster of active makers, each with its position count and shared liquidity.
#[utoipa::path(get, path = "/v1/makers", responses((status = 200, body = Response<List<MakerSummary>>)))]
pub async fn makers(State(state): State<AppState>) -> ApiResult<List<MakerSummary>> {
    Ok(Response::ok(List::all(state.makers.roster().await?)))
}

/// One maker's dashboard (KPIs, fill-share, latency, insight). `{maker}` is an address (`me` needs a
/// wallet).
#[utoipa::path(
    get,
    path = "/v1/makers/{maker}",
    params(("maker" = String, Path, description = "Maker address")),
    responses((status = 200, body = Response<MakerDashboard>))
)]
pub async fn maker_dashboard(
    State(state): State<AppState>,
    Path(maker): Path<String>,
) -> ApiResult<MakerDashboard> {
    let maker = MakerId(parse_maker(&maker)?);
    Ok(Response::ok(state.makers.dashboard(maker).await?))
}

/// A maker's active positions (list projection). `{maker}` is an address (`me` needs a wallet).
#[utoipa::path(
    get,
    path = "/v1/makers/{maker}/positions",
    params(("maker" = String, Path, description = "Maker address")),
    responses((status = 200, body = Response<List<Position>>))
)]
pub async fn maker_positions(
    State(state): State<AppState>,
    Path(maker): Path<String>,
) -> ApiResult<List<Position>> {
    let maker = MakerId(parse_maker(&maker)?);
    Ok(Response::ok(List::all(
        state.makers.positions(maker).await?,
    )))
}

/// One position's full detail, or `404` if the hash is unknown.
#[utoipa::path(
    get,
    path = "/v1/positions/{hash}",
    params(("hash" = String, Path, description = "Strategy hash (position id)")),
    responses(
        (status = 200, body = Response<Position>),
        (status = 404, description = "Unknown position"),
    )
)]
pub async fn position_detail(
    State(state): State<AppState>,
    Path(hash): Path<String>,
) -> ApiResult<Position> {
    let hash = StrategyHash(hash.parse::<B256>().map_err(|e| SolventError::InvalidId {
        id_type: "strategy_hash",
        reason: e.to_string(),
    })?);
    match state.makers.position(hash).await? {
        Some(position) => Ok(Response::ok(position)),
        None => Err(Response::error("position not found", StatusCode::NOT_FOUND)),
    }
}

fn parse_maker(s: &str) -> Result<Address, SolventError> {
    if s == "me" {
        return Err(SolventError::InvalidId {
            id_type: "maker",
            reason: "\"me\" requires a connected wallet".to_string(),
        });
    }
    s.parse::<Address>().map_err(|e| SolventError::InvalidId {
        id_type: "maker",
        reason: e.to_string(),
    })
}
