//! `GET /v1/config` — the FE bootstrap: chain id, feature flags, defaults, explorer URL.

use axum::extract::State;

use crate::http::primitives::Response;
use crate::http::state::{AppConfig, AppState};

/// Returns the runtime config verbatim; the FE reads it once at startup. Infallible (an in-memory
/// clone), so it returns `Response` directly rather than `ApiResult`.
#[utoipa::path(get, path = "/v1/config", responses((status = 200, body = Response<AppConfig>)))]
pub async fn config(State(state): State<AppState>) -> Response<AppConfig> {
    Response::ok((*state.config).clone())
}
