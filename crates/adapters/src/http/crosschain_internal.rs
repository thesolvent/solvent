//! Private chain-service API. Deployments bind this router separately from browser-facing routes.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use solvent_core::crosschain::{LocalCrossChainService, LocalStepService};
use solvent_core::deps::crosschain::RemoteProgress;
use solvent_core::primitives::crosschain::{
    ChainExecutionPlan, LegQuote, LegQuoteRequest, Preparation, RemoteCommand,
};
use solvent_core::primitives::{
    AggregateQuoteId, CrossChainOrderId, CrossChainStepId, PrepareToken, SolventError,
};

#[derive(Clone)]
struct InternalState {
    local: Arc<LocalCrossChainService>,
    steps: Arc<LocalStepService>,
}

#[derive(Clone)]
struct InternalAuth {
    authorization: HeaderValue,
}

pub fn crosschain_internal_router(
    local: Arc<LocalCrossChainService>,
    steps: Arc<LocalStepService>,
    authorization: HeaderValue,
) -> Router {
    let state = InternalState { local, steps };
    let auth = InternalAuth { authorization };
    Router::new()
        .route("/internal/v1/cross-chain/leg-quotes", post(leg_quote))
        .route("/internal/v1/cross-chain/stage", post(stage))
        .route("/internal/v1/cross-chain/preparations", post(prepare))
        .route("/internal/v1/cross-chain/preparations/commit", post(commit))
        .route(
            "/internal/v1/cross-chain/preparations/inspect",
            post(inspect),
        )
        .route(
            "/internal/v1/cross-chain/preparations/release",
            post(release),
        )
        .route("/internal/v1/cross-chain/steps", post(command))
        .with_state(state)
        .layer(middleware::from_fn_with_state(auth, require_auth))
}

async fn require_auth(
    State(auth): State<InternalAuth>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let supplied = request.headers().get(axum::http::header::AUTHORIZATION);
    if supplied != Some(&auth.authorization) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}

#[derive(Deserialize)]
struct StageRequest {
    order_id: CrossChainOrderId,
    plan: ChainExecutionPlan,
}

#[derive(Deserialize)]
struct PrepareRequest {
    aggregate_id: AggregateQuoteId,
    quote: LegQuote,
}

#[derive(Deserialize)]
struct TokenRequest {
    token: PrepareToken,
}

#[derive(Deserialize)]
struct CommandRequest {
    order_id: CrossChainOrderId,
    command_id: CrossChainStepId,
    command: RemoteCommand,
    preparation: PrepareToken,
}

async fn leg_quote(
    State(state): State<InternalState>,
    Json(request): Json<LegQuoteRequest>,
) -> Result<Json<LegQuote>, StatusCode> {
    state
        .local
        .quote(&request)
        .await
        .map(Json)
        .map_err(internal_error)
}

async fn stage(
    State(state): State<InternalState>,
    Json(request): Json<StageRequest>,
) -> Result<Json<()>, StatusCode> {
    state
        .steps
        .stage(request.order_id, &request.plan)
        .await
        .map(|()| Json(()))
        .map_err(internal_error)
}

async fn prepare(
    State(state): State<InternalState>,
    Json(request): Json<PrepareRequest>,
) -> Result<Json<Preparation>, StatusCode> {
    state
        .local
        .prepare(request.aggregate_id, &request.quote)
        .await
        .map(Json)
        .map_err(internal_error)
}

async fn commit(
    State(state): State<InternalState>,
    Json(request): Json<TokenRequest>,
) -> Result<Json<Preparation>, StatusCode> {
    state
        .local
        .commit(request.token)
        .await
        .map(Json)
        .map_err(internal_error)
}

async fn release(
    State(state): State<InternalState>,
    Json(request): Json<TokenRequest>,
) -> Result<Json<Preparation>, StatusCode> {
    state
        .local
        .release(request.token)
        .await
        .map(Json)
        .map_err(internal_error)
}

async fn inspect(
    State(state): State<InternalState>,
    Json(request): Json<TokenRequest>,
) -> Result<Json<Preparation>, StatusCode> {
    state
        .local
        .inspect(request.token)
        .await
        .map(Json)
        .map_err(internal_error)
}

async fn command(
    State(state): State<InternalState>,
    Json(request): Json<CommandRequest>,
) -> Result<Json<RemoteProgress>, StatusCode> {
    let progress = state
        .steps
        .command(request.order_id, request.command_id, request.command)
        .await
        .map_err(internal_error)?;
    if matches!(progress, RemoteProgress::Finalized { .. })
        && matches!(
            request.command,
            RemoteCommand::Deliver | RemoteCommand::ClaimOrigin
        )
    {
        state
            .local
            .mark_executed(request.preparation)
            .await
            .map_err(internal_error)?;
    }
    Ok(Json(progress))
}

fn internal_error(error: SolventError) -> StatusCode {
    match error {
        SolventError::InvalidCrossChain(_) | SolventError::InvalidId { .. } => {
            StatusCode::BAD_REQUEST
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use tower::ServiceExt;

    fn protected() -> Router {
        let mut authorization = HeaderValue::from_static("Bearer private-test-token");
        authorization.set_sensitive(true);
        Router::new()
            .route("/protected", get(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(
                InternalAuth { authorization },
                require_auth,
            ))
    }

    #[tokio::test]
    async fn private_routes_require_the_exact_bearer_credential() {
        let missing = protected()
            .clone()
            .oneshot(Request::get("/protected").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let accepted = protected()
            .oneshot(
                Request::get("/protected")
                    .header("authorization", "Bearer private-test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::OK);
    }
}
