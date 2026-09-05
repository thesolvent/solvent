//! `GET /v1/config` — the FE bootstrap: chain id, feature flags, defaults, explorer URL.

use axum::extract::State;

use crate::http::primitives::{ApiResult, Response};
use crate::http::state::{AppConfig, AppState};

/// Returns the runtime config verbatim; the FE reads it once at startup.
#[utoipa::path(get, path = "/v1/config", responses((status = 200, body = Response<AppConfig>)))]
pub async fn config(State(state): State<AppState>) -> ApiResult<AppConfig> {
    Ok(Response::ok((*state.config).clone()))
}
