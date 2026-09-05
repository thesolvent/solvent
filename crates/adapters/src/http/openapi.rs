//! The generated OpenAPI document, served at `/v1/openapi.json`. Paths and schemas are collected
//! from the handlers' `#[utoipa::path]` annotations and the `ToSchema` derives, so the document
//! cannot drift from the code.

use axum::Json;
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(title = "Solvent API", version = "0.1.0"),
    paths(
        crate::http::handlers::config::config,
        crate::http::handlers::stats::stats,
        crate::http::handlers::assets::assets,
    ),
    components(schemas(
        crate::http::primitives::Status,
        crate::http::state::AppConfig,
        crate::http::state::Features,
        crate::http::handlers::stats::Stats,
        solvent_core::asset::Asset,
    ))
)]
pub struct ApiDoc;

/// Serve the OpenAPI document as JSON.
pub async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}
