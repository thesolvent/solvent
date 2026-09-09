//! `GET /v1/pairs` — the tradeable pairs for the Create wizard. `?search=` filters by symbol or
//! address; `?wallet=` adds the maker's per-side balance. All assembly is in the `AssetManager`.

use axum::extract::{Query, State};
use serde::Deserialize;
use solvent_core::asset::{PairInfo, PairPriceHistory, PriceHistoryPeriod};

use crate::http::dto::List;
use crate::http::primitives::{parse_addr, ApiResult, Response};
use crate::http::state::AppState;

/// The default band (± percent) the wizard suggests when a pair has no better hint.
const DEFAULT_BAND_PCT: f64 = 5.0;

#[derive(Debug, Deserialize)]
pub struct PairsQuery {
    search: Option<String>,
    wallet: Option<String>,
}

#[utoipa::path(
    get,
    path = "/v1/pairs",
    params(
        ("search" = Option<String>, Query, description = "Filter by token symbol or address"),
        ("wallet" = Option<String>, Query, description = "Include this wallet's per-side balances"),
    ),
    responses((status = 200, body = Response<List<PairInfo>>))
)]
pub async fn pairs(
    State(state): State<AppState>,
    Query(query): Query<PairsQuery>,
) -> ApiResult<List<PairInfo>> {
    let wallet = match query.wallet {
        Some(ref addr) => Some(state.balances.balances(parse_addr("wallet", addr)?).await?),
        None => None,
    };
    let pairs = state
        .assets
        .pairs(
            &state.valuation,
            wallet.as_deref(),
            query.search.as_deref(),
            state.config.default_fee_bps,
            DEFAULT_BAND_PCT,
        )
        .await;
    Ok(Response::ok(List::all(pairs)))
}

#[derive(Debug, Deserialize)]
pub struct PairHistoryQuery {
    base: String,
    quote: String,
    period: PriceHistoryPeriod,
}

#[utoipa::path(
    get,
    path = "/v1/pairs/history",
    params(
        ("base" = String, Query, description = "Base token address"),
        ("quote" = String, Query, description = "Quote token address"),
        ("period" = PriceHistoryPeriod, Query, description = "Historical chart window"),
    ),
    responses((status = 200, body = Response<PairPriceHistory>))
)]
pub async fn pair_history(
    State(state): State<AppState>,
    Query(query): Query<PairHistoryQuery>,
) -> ApiResult<PairPriceHistory> {
    let base = parse_addr("base", &query.base)?;
    let quote = parse_addr("quote", &query.quote)?;
    Ok(Response::ok(
        state
            .pair_history
            .history(base, quote, query.period)
            .await?,
    ))
}
