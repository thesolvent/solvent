//! Browser-facing cross-chain proxy API. The proxy has no RPC provider or signing key.

use std::sync::Arc;

use alloy::primitives::{Address, B256, U256};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use solvent_core::crosschain::CrossChainProxy;
use solvent_core::deps::ledger::Clock;
use solvent_core::primitives::crosschain::{
    AggregateQuote, ChainExecutionPlan, CrossChainRoute, CrossChainSaga, LegQuoteRequest, LegRole,
};
use solvent_core::primitives::{ChainId, CrossChainOrderId, SolventError};
use utoipa::OpenApi;

#[derive(Clone)]
struct ProxyState {
    proxy: Arc<CrossChainProxy>,
    clock: Arc<dyn Clock>,
}

pub fn crosschain_proxy_router(proxy: Arc<CrossChainProxy>, clock: Arc<dyn Clock>) -> Router {
    let state = ProxyState { proxy, clock };
    Router::new()
        .merge(documentation_routes())
        .route("/healthz", get(healthz))
        .route("/v1/cross-chain/quote", post(quote))
        .route("/v1/cross-chain/orders", post(create_order))
        .route("/v1/cross-chain/orders/{id}", get(order))
        .route("/v1/cross-chain/orders/{id}/advance", post(advance))
        .with_state(state)
}

fn documentation_routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/v1/x/openapi.json", get(openapi))
        .route("/v1/openapi.json", get(openapi))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CrossChainQuoteRequest {
    #[schema(value_type = String)]
    pub request_id: B256,
    pub origin_chain_id: u64,
    pub destination_chain_id: u64,
    #[schema(value_type = String)]
    pub origin_token_in: Address,
    #[schema(value_type = String)]
    pub origin_token_out: Address,
    #[schema(value_type = String)]
    pub destination_token_in: Address,
    #[schema(value_type = String)]
    pub destination_token_out: Address,
    #[schema(value_type = String)]
    pub amount_in: U256,
    #[schema(value_type = String)]
    pub destination_amount_in: U256,
    pub deadline_unix: u64,
    pub route: CrossChainRoute,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateCrossChainOrderRequest {
    #[schema(value_type = String)]
    pub order_id: CrossChainOrderId,
    pub quote: AggregateQuote,
    pub origin_plan: ChainExecutionPlan,
    pub destination_plan: ChainExecutionPlan,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CrossChainOrderResponse {
    pub order: CrossChainSaga,
}

/// Join the independently authoritative origin and destination quotes.
#[utoipa::path(
    post,
    path = "/v1/cross-chain/quote",
    tag = "cross-chain",
    request_body = CrossChainQuoteRequest,
    responses(
        (status = 200, description = "Aggregate quote accepted by both chain services", body = AggregateQuote),
        (status = 400, description = "Invalid route, chain pair, or amount", body = ErrorBody),
        (status = 502, description = "A chain service is unavailable", body = ErrorBody),
    )
)]
async fn quote(
    State(state): State<ProxyState>,
    Json(request): Json<CrossChainQuoteRequest>,
) -> Result<Json<AggregateQuote>, (StatusCode, Json<ErrorBody>)> {
    if request.origin_chain_id == request.destination_chain_id
        || request.amount_in.is_zero()
        || request.destination_amount_in.is_zero()
    {
        return Err(client_error(
            "chains must differ and amounts must be non-zero",
        ));
    }
    let origin = LegQuoteRequest {
        request_id: request.request_id,
        role: LegRole::Origin,
        local_chain: ChainId(request.origin_chain_id),
        remote_chain: ChainId(request.destination_chain_id),
        input_token: request.origin_token_in,
        output_token: request.origin_token_out,
        amount: request.amount_in,
        deadline_unix: request.deadline_unix,
        route: request.route,
    };
    let destination = LegQuoteRequest {
        request_id: request.request_id,
        role: LegRole::Destination,
        local_chain: ChainId(request.destination_chain_id),
        remote_chain: ChainId(request.origin_chain_id),
        input_token: request.destination_token_in,
        output_token: request.destination_token_out,
        amount: request.destination_amount_in,
        deadline_unix: request.deadline_unix,
        route: request.route,
    };
    state
        .proxy
        .quote(&origin, &destination, state.clock.now_unix())
        .await
        .map(Json)
        .map_err(proxy_error)
}

/// Validate both execution plans, prepare capital on each chain, and start the durable saga.
#[utoipa::path(
    post,
    path = "/v1/cross-chain/orders",
    tag = "cross-chain",
    request_body = CreateCrossChainOrderRequest,
    responses(
        (status = 200, description = "Cross-chain order created or idempotently recovered", body = CrossChainOrderResponse),
        (status = 400, description = "Quote or execution plan is invalid", body = ErrorBody),
        (status = 502, description = "A chain service or saga store is unavailable", body = ErrorBody),
    )
)]
async fn create_order(
    State(state): State<ProxyState>,
    Json(request): Json<CreateCrossChainOrderRequest>,
) -> Result<Json<CrossChainOrderResponse>, (StatusCode, Json<ErrorBody>)> {
    state
        .proxy
        .start(
            request.order_id,
            request.quote,
            &request.origin_plan,
            &request.destination_plan,
            state.clock.now_unix(),
        )
        .await
        .map(|order| Json(CrossChainOrderResponse { order }))
        .map_err(proxy_error)
}

/// Return the last durable state recorded for a cross-chain order.
#[utoipa::path(
    get,
    path = "/v1/cross-chain/orders/{id}",
    tag = "cross-chain",
    params(
        ("id" = String, Path, description = "32-byte cross-chain order identifier")
    ),
    responses(
        (status = 200, description = "Current durable order state", body = CrossChainOrderResponse),
        (status = 400, description = "Malformed or unknown order identifier", body = ErrorBody),
        (status = 502, description = "The saga store is unavailable", body = ErrorBody),
    )
)]
async fn order(
    State(state): State<ProxyState>,
    Path(order_id): Path<CrossChainOrderId>,
) -> Result<Json<CrossChainOrderResponse>, (StatusCode, Json<ErrorBody>)> {
    state
        .proxy
        .status(order_id)
        .await
        .map(|order| Json(CrossChainOrderResponse { order }))
        .map_err(proxy_error)
}

/// Advance one durable cross-chain saga transition.
#[utoipa::path(
    post,
    path = "/v1/cross-chain/orders/{id}/advance",
    tag = "cross-chain",
    params(
        ("id" = String, Path, description = "32-byte cross-chain order identifier")
    ),
    responses(
        (status = 200, description = "Saga after at most one advancement", body = CrossChainOrderResponse),
        (status = 400, description = "Malformed or unknown order identifier", body = ErrorBody),
        (status = 502, description = "A chain service or saga store is unavailable", body = ErrorBody),
    )
)]
async fn advance(
    State(state): State<ProxyState>,
    Path(order_id): Path<CrossChainOrderId>,
) -> Result<Json<CrossChainOrderResponse>, (StatusCode, Json<ErrorBody>)> {
    state
        .proxy
        .advance(order_id)
        .await
        .map(|order| Json(CrossChainOrderResponse { order }))
        .map_err(proxy_error)
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
struct ErrorBody {
    error: String,
}

fn proxy_error(error: SolventError) -> (StatusCode, Json<ErrorBody>) {
    match error {
        SolventError::InvalidCrossChain(message) => client_error(&message),
        _ => (
            StatusCode::BAD_GATEWAY,
            Json(ErrorBody {
                error: "cross-chain service unavailable".to_string(),
            }),
        ),
    }
}

fn client_error(message: &str) -> (StatusCode, Json<ErrorBody>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorBody {
            error: message.to_string(),
        }),
    )
}

async fn healthz() -> &'static str {
    "ok"
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Solvent Cross-Chain Proxy API", version = "0.1.0"),
    paths(quote, create_order, order, advance),
    components(schemas(
        CrossChainQuoteRequest,
        CreateCrossChainOrderRequest,
        CrossChainOrderResponse,
        ErrorBody,
        CrossChainRoute,
        LegRole,
        solvent_core::primitives::crosschain::RemoteCommand,
        solvent_core::primitives::crosschain::PreparedStep,
        ChainExecutionPlan,
        solvent_core::primitives::ledger::ReservationSource,
        solvent_core::primitives::crosschain::LegQuote,
        AggregateQuote,
        solvent_core::primitives::crosschain::SagaState,
        solvent_core::primitives::crosschain::StepEvidence,
        CrossChainSaga,
    ))
)]
pub struct CrossChainApiDoc;

async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(CrossChainApiDoc::openapi())
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn crosschain_openapi_is_served_on_canonical_and_legacy_paths() {
        let canonical = document("/v1/x/openapi.json").await;
        let legacy = document("/v1/openapi.json").await;

        assert_eq!(canonical, legacy);
        assert_eq!(canonical["openapi"], "3.1.0");
        assert!(canonical["paths"]["/v1/cross-chain/quote"]["post"].is_object());
        assert!(canonical["paths"]["/v1/cross-chain/orders"]["post"].is_object());
        assert!(canonical["paths"]["/v1/cross-chain/orders/{id}"]["get"].is_object());
        assert!(canonical["paths"]["/v1/cross-chain/orders/{id}/advance"]["post"].is_object());
        assert!(canonical["components"]["schemas"]["CrossChainSaga"].is_object());
    }

    async fn document(path: &str) -> serde_json::Value {
        let response = documentation_routes::<()>()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
}
