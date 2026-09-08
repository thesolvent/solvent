//! `GET /v1/makers` — the active-maker roster. `GET /v1/makers/{maker}` — one maker's dashboard.
//! `GET /v1/makers/{maker}/positions` — a maker's positions (list projection). `GET
//! /v1/positions/{hash}` — one position's full detail.

use crate::ledger::SystemClock;
use alloy::primitives::{Address, B256};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use solvent_core::deps::ledger::Clock;
use solvent_core::deps::trade::Page as StorePage;
use solvent_core::primitives::maker::{
    InventoryRow, MakerDashboard, MakerPeriod, MakerSummary, Position, PositionHistory,
};
use solvent_core::primitives::trade::{MakerTrade, TradeId};
use solvent_core::primitives::{MakerId, StrategyHash};
use solvent_core::SolventError;

use crate::http::dto::{Cursor, List, Page};
use crate::http::primitives::{ApiResult, Response};
use crate::http::state::AppState;

/// A rolling period for dashboard and position economics.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct MakerPeriodQuery {
    #[serde(default)]
    period: MakerPeriod,
}

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
    params(("maker" = String, Path, description = "Maker address"), MakerPeriodQuery),
    responses((status = 200, body = Response<MakerDashboard>))
)]
pub async fn maker_dashboard(
    State(state): State<AppState>,
    Path(maker): Path<String>,
    Query(query): Query<MakerPeriodQuery>,
) -> ApiResult<MakerDashboard> {
    let maker = MakerId(parse_maker(&maker)?);
    Ok(Response::ok(
        state.makers.dashboard(maker, query.period).await?,
    ))
}

/// A maker's inventory: its positions re-grouped by token (the Assets tab). `{maker}` is an address.
#[utoipa::path(
    get,
    path = "/v1/makers/{maker}/inventory",
    params(("maker" = String, Path, description = "Maker address"), MakerPeriodQuery),
    responses((status = 200, body = Response<List<InventoryRow>>))
)]
pub async fn maker_inventory(
    State(state): State<AppState>,
    Path(maker): Path<String>,
    Query(query): Query<MakerPeriodQuery>,
) -> ApiResult<List<InventoryRow>> {
    let maker = MakerId(parse_maker(&maker)?);
    Ok(Response::ok(List::all(
        state.makers.inventory(maker, query.period).await?,
    )))
}

/// Pagination for the maker settlements feed.
#[derive(Debug, Deserialize)]
pub struct MakerTradesQuery {
    period: Option<MakerPeriod>,
    limit: Option<u32>,
    cursor: Option<String>,
}

/// A maker's settlements (newest first, cursor-paginated) — its trades with per-trade share and fee.
#[utoipa::path(
    get,
    path = "/v1/makers/{maker}/trades",
    params(
        ("maker" = String, Path, description = "Maker address"),
        ("limit" = Option<u32>, Query, description = "Page size (default 50, max 200)"),
        ("cursor" = Option<String>, Query, description = "Opaque next-page cursor"),
        ("period" = Option<MakerPeriod>, Query, description = "Confirmed settlements in a rolling period"),
    ),
    responses((status = 200, body = Response<List<MakerTrade>>))
)]
pub async fn maker_trades(
    State(state): State<AppState>,
    Path(maker): Path<String>,
    Query(query): Query<MakerTradesQuery>,
) -> ApiResult<List<MakerTrade>> {
    let maker = parse_maker(&maker)?;
    let page = Page {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let cursor = page
        .cursor::<String>()?
        .map(|id| id.parse::<TradeId>())
        .transpose()?;
    let store_page = StorePage {
        limit: page.limit(),
        cursor,
    };
    let items = state
        .trades
        .maker_trades(
            maker,
            &store_page,
            query.period.map(|p| p.window(SystemClock.now_unix())),
        )
        .await?;
    let next_cursor = (items.len() as u32 == store_page.limit)
        .then(|| items.last().map(|t| Cursor::encode(&t.trade.id)))
        .flatten();
    Ok(Response::ok(List::page(items, next_cursor, None)))
}

/// A maker's active positions (list projection). `{maker}` is an address (`me` needs a wallet).
#[utoipa::path(
    get,
    path = "/v1/makers/{maker}/positions",
    params(("maker" = String, Path, description = "Maker address"), MakerPeriodQuery),
    responses((status = 200, body = Response<List<Position>>))
)]
pub async fn maker_positions(
    State(state): State<AppState>,
    Path(maker): Path<String>,
    Query(query): Query<MakerPeriodQuery>,
) -> ApiResult<List<Position>> {
    let maker = MakerId(parse_maker(&maker)?);
    Ok(Response::ok(List::all(
        state.makers.positions(maker, query.period).await?,
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

/// The strategy's committed-reserve price over the last seven days.
#[utoipa::path(get, path = "/v1/positions/{hash}/history",
    params(("hash" = String, Path, description = "Strategy hash")),
    responses((status = 200, body = Response<PositionHistory>), (status = 404, description = "Unknown position")))]
pub async fn position_history(
    State(state): State<AppState>,
    Path(hash): Path<String>,
) -> ApiResult<PositionHistory> {
    let hash = StrategyHash(hash.parse::<B256>().map_err(|e| SolventError::InvalidId {
        id_type: "strategy_hash",
        reason: e.to_string(),
    })?);
    match state.makers.history(hash).await? {
        Some(history) => Ok(Response::ok(history)),
        None => Err(Response::error("position not found", StatusCode::NOT_FOUND)),
    }
}
