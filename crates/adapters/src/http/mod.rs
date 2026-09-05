//! Inbound HTTP adapter: the response envelope, error mapping, and the axum router.
//!
//! Handlers stay thin — they translate HTTP to/from the domain and return [`primitives::ApiResult`];
//! business logic lives in core services. Business routes mount under `/v1` as milestones land.

pub mod dto;
pub mod error;
pub mod primitives;

use std::time::Duration;

use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Wall-clock ceiling for any request; devnet-generous, tuned per deployment later.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The HTTP router: the middleware stack (request-id → trace → timeout → CORS) wrapping the routes.
/// `/healthz` is the liveness probe.
pub fn router() -> Router {
    Router::new().route("/healthz", get(healthz)).layer(
        ServiceBuilder::new()
            .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
            .layer(TraceLayer::new_for_http())
            .layer(PropagateRequestIdLayer::x_request_id())
            .layer(TimeoutLayer::with_status_code(
                StatusCode::REQUEST_TIMEOUT,
                REQUEST_TIMEOUT,
            ))
            .layer(CorsLayer::permissive()),
    )
}

async fn healthz() -> &'static str {
    "ok"
}
