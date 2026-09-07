//! `GET /v1/assets` — the asset catalog. `?supported=true` returns only assets with active
//! liquidity (the public export); omit it for the full catalog. All info comes from the
//! `AssetManager` — the handler never touches raw metadata.

use axum::extract::{Query, State};
use serde::Deserialize;
use solvent_core::asset::Asset;

use crate::http::dto::List;
use crate::http::primitives::Response;
use crate::http::state::AppState;

#[derive(Debug, Deserialize)]
pub struct AssetsQuery {
    supported: Option<bool>,
}

#[utoipa::path(
    get,
    path = "/v1/assets",
    params(("supported" = Option<bool>, Query, description = "Only assets with active liquidity")),
    responses((status = 200, body = Response<List<Asset>>))
)]
pub async fn assets(
    State(state): State<AppState>,
    Query(query): Query<AssetsQuery>,
) -> Response<List<Asset>> {
    Response::ok(List::all(
        state
            .assets
            .list(query.supported.unwrap_or(false), &state.valuation)
            .await,
    ))
}
