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
use solvent_core::deps::crosschain::{DirectPlanAuthor, LegQuoterError, RemoteProgress};
use solvent_core::primitives::crosschain::{
    ChainExecutionPlan, DirectExecutionPlans, DirectOrderAuthorization, LegQuote, LegQuoteRequest,
    Preparation, RemoteCommand, StepValidationContext,
};
use solvent_core::primitives::{
    AggregateQuoteId, CrossChainOrderId, CrossChainStepId, PrepareToken, SolventError,
};

#[derive(Clone)]
struct InternalState {
    local: Arc<LocalCrossChainService>,
    steps: Arc<LocalStepService>,
    direct_author: Option<Arc<dyn DirectPlanAuthor>>,
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
    crosschain_internal_router_with_author(local, steps, authorization, None)
}

pub fn crosschain_internal_router_with_author(
    local: Arc<LocalCrossChainService>,
    steps: Arc<LocalStepService>,
    authorization: HeaderValue,
    direct_author: Option<Arc<dyn DirectPlanAuthor>>,
) -> Router {
    let state = InternalState {
        local,
        steps,
        direct_author,
    };
    let auth = InternalAuth { authorization };
    Router::new()
        .route("/internal/v1/cross-chain/leg-quotes", post(leg_quote))
        .route("/internal/v1/cross-chain/stage", post(stage))
        .route("/internal/v1/cross-chain/direct-plans", post(author_direct))
        .route(
            "/internal/v1/cross-chain/stage-cctp-completion",
            post(stage_cctp_completion),
        )
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

async fn author_direct(
    State(state): State<InternalState>,
    Json(authorization): Json<DirectOrderAuthorization>,
) -> Result<Json<DirectExecutionPlans>, ErrorReply> {
    let author = state.direct_author.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "direct settlement is not configured on this deployment".to_string(),
        )
    })?;
    author
        .author(&authorization)
        .await
        .map(Json)
        .map_err(|error| match error {
            solvent_core::deps::crosschain::DirectPlanAuthorError::Invalid(reason) => {
                (StatusCode::BAD_REQUEST, reason)
            }
            solvent_core::deps::crosschain::DirectPlanAuthorError::Unavailable(reason) => {
                (StatusCode::INTERNAL_SERVER_ERROR, reason)
            }
        })
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
    context: StepValidationContext,
    plan: ChainExecutionPlan,
}

#[derive(Deserialize)]
struct CompletionStageRequest {
    context: StepValidationContext,
    plan: ChainExecutionPlan,
    preparation: PrepareToken,
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
) -> Result<Json<LegQuote>, ErrorReply> {
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
) -> Result<Json<()>, ErrorReply> {
    state
        .local
        .validate_stage_context(&request.context)
        .await
        .map_err(internal_error)?;
    state
        .steps
        .stage(&request.context, &request.plan)
        .await
        .map(|()| Json(()))
        .map_err(internal_error)
}

async fn stage_cctp_completion(
    State(state): State<InternalState>,
    Json(request): Json<CompletionStageRequest>,
) -> Result<Json<()>, ErrorReply> {
    state
        .local
        .validate_stage_context(&request.context)
        .await
        .map_err(internal_error)?;
    let preparation = state
        .local
        .inspect(request.preparation)
        .await
        .map_err(internal_error)?;
    state
        .steps
        .stage_cctp_completion(&request.context, &request.plan, &preparation)
        .await
        .map(|()| Json(()))
        .map_err(internal_error)
}

async fn prepare(
    State(state): State<InternalState>,
    Json(request): Json<PrepareRequest>,
) -> Result<Json<Preparation>, ErrorReply> {
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
) -> Result<Json<Preparation>, ErrorReply> {
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
) -> Result<Json<Preparation>, ErrorReply> {
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
) -> Result<Json<Preparation>, ErrorReply> {
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
) -> Result<Json<RemoteProgress>, ErrorReply> {
    let preparation = state
        .local
        .inspect(request.preparation)
        .await
        .map_err(internal_error)?;
    let progress = state
        .steps
        .command(
            request.order_id,
            request.command_id,
            request.command,
            &preparation,
        )
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

/// A refusal reaches the caller in the words the service used, not as a bare status.
///
/// The proxy is several hops from the browser, and each hop that keeps only a status code turns a
/// precise refusal into an outage: "beyond the book" was arriving as "service unavailable".
type ErrorReply = (StatusCode, String);

fn internal_error(error: SolventError) -> ErrorReply {
    let status = match error {
        SolventError::InvalidCrossChain(_)
        | SolventError::StepValidator(_)
        | SolventError::InvalidId { .. } => StatusCode::BAD_REQUEST,
        // Not a failure: this pair and size simply cannot be filled from the book right now.
        SolventError::LegQuote(LegQuoterError::NoRoute) => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, error.to_string())
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
