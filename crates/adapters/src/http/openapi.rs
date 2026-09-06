//! The generated OpenAPI document, served at `/v1/openapi.json`. Paths and schemas are collected
//! from the handlers' `#[utoipa::path]` annotations and the `ToSchema` derives, so the document
//! cannot drift from the code.

use axum::Json;
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(title = "Solvent API", version = "0.1.0"),
    paths(
        crate::http::app::config::config,
        crate::http::app::stats::stats,
        crate::http::app::assets::assets,
        crate::http::app::pools::pools,
        crate::http::app::pools::pool_detail,
        crate::http::app::pools::pool_depth,
        crate::http::app::balances::balances,
        crate::http::app::swap::quote,
        crate::http::app::swap::submit,
    ),
    components(schemas(
        crate::http::primitives::Status,
        crate::http::state::AppConfig,
        crate::http::state::Features,
        crate::http::app::stats::Stats,
        solvent_core::asset::Asset,
        solvent_core::asset::Token,
        solvent_core::pool::Pool,
        solvent_core::pool::PoolType,
        solvent_core::pool::PoolMaker,
        solvent_core::pool::PoolDetail,
        solvent_core::pool::PoolDepth,
        solvent_core::pool::DepthPoint,
        solvent_core::balances::TokenBalance,
        solvent_core::primitives::amount::Amount,
        solvent_core::primitives::amount::TokenAmount,
        solvent_core::primitives::amount::TokenAmounts,
        solvent_core::quote::QuoteResponse,
        solvent_core::quote::QuoteLeg,
        crate::http::app::swap::QuoteRequest,
        crate::http::app::swap::SwapRequest,
        crate::http::app::swap::SwapResponse,
    ))
)]
pub struct ApiDoc;

/// Serve the OpenAPI document as JSON.
pub async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}
