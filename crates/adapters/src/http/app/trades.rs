//! `GET /v1/trades` — the trade list (filtered, cursor-paginated), and `GET /v1/trades/{id}` — one
//! trade's full lifecycle. Assembly lives in the core `TradeService`; the handlers only parse the
//! query filter and frame HTTP pagination.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use solvent_core::deps::trade::{Page as StorePage, TradeFilter};
use solvent_core::primitives::trade::{TradeId, TradeView};

use crate::http::dto::{Cursor, List, Page};
use crate::http::primitives::{parse_addr, ApiResult, Response};
use crate::http::state::AppState;

/// List + filter query: pagination (`limit`/`cursor`) and the header filters (`status`, `taker`,
/// and a `base`+`quote` pair, order-independent).
#[derive(Debug, Deserialize)]
pub struct TradesQuery {
    limit: Option<u32>,
    cursor: Option<String>,
    status: Option<String>,
    taker: Option<String>,
    base: Option<String>,
    quote: Option<String>,
}

/// List trades, newest first, filtered and cursor-paginated.
#[utoipa::path(
    get,
    path = "/v1/trades",
    params(
        ("limit" = Option<u32>, Query, description = "Page size (default 50, max 200)"),
        ("cursor" = Option<String>, Query, description = "Opaque next-page cursor"),
        ("status" = Option<String>, Query, description = "Lifecycle status filter"),
        ("taker" = Option<String>, Query, description = "Swapper address filter"),
        ("base" = Option<String>, Query, description = "Pair token (with quote)"),
        ("quote" = Option<String>, Query, description = "Pair token (with base)"),
    ),
    responses((status = 200, body = Response<List<TradeView>>))
)]
pub async fn trades(
    State(state): State<AppState>,
    Query(query): Query<TradesQuery>,
) -> ApiResult<List<TradeView>> {
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
    let filter = TradeFilter {
        status: query.status.as_deref().map(str::parse).transpose()?,
        taker: query
            .taker
            .as_deref()
            .map(|s| parse_addr("token", s))
            .transpose()?,
        pair: match (query.base.as_deref(), query.quote.as_deref()) {
            (Some(base), Some(quote)) => {
                Some((parse_addr("token", base)?, parse_addr("token", quote)?))
            }
            _ => None,
        },
    };

    let items = state.trades.list(&filter, &store_page).await?;
    // A full page implies there may be more; the cursor is the last id seen.
    let next_cursor = (items.len() as u32 == store_page.limit)
        .then(|| items.last().map(|t| Cursor::encode(&t.id)))
        .flatten();
    Ok(Response::ok(List::page(items, next_cursor, None)))
}

/// One trade's full detail, or `404` if the id is unknown.
#[utoipa::path(
    get,
    path = "/v1/trades/{id}",
    params(("id" = String, Path, description = "Trade id (ULID)")),
    responses(
        (status = 200, body = Response<TradeView>),
        (status = 404, description = "Unknown trade"),
    )
)]
pub async fn trade_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<TradeView> {
    let id = id.parse::<TradeId>()?;
    match state.trades.detail(&id).await? {
        Some(view) => Ok(Response::ok(view)),
        None => Err(Response::error("trade not found", StatusCode::NOT_FOUND)),
    }
}
