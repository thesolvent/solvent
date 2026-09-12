//! Browser-facing cross-chain proxy API. The proxy has no RPC provider or signing key.

use std::sync::Arc;

use crate::crosschain::{
    CompactCommitmentTerms, DirectOrderDraft, DirectOrderDraftBuilder, DirectOrderDraftRequest,
    SolventCompactMandate, SolventCompactOrder,
};
use alloy::primitives::{Address, Bytes, B256, U256};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use solvent_core::crosschain::CrossChainProxy;
use solvent_core::deps::crosschain::RemoteSolventError;
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
    direct_drafts: Option<DirectOrderDraftBuilder>,
}

pub fn crosschain_proxy_router(proxy: Arc<CrossChainProxy>, clock: Arc<dyn Clock>) -> Router {
    crosschain_proxy_router_with_drafts(proxy, clock, None)
}

pub fn crosschain_proxy_router_with_drafts(
    proxy: Arc<CrossChainProxy>,
    clock: Arc<dyn Clock>,
    direct_drafts: Option<DirectOrderDraftBuilder>,
) -> Router {
    let state = ProxyState {
        proxy,
        clock,
        direct_drafts,
    };
    Router::new()
        .merge(documentation_routes())
        .route("/healthz", get(healthz))
        .route("/v1/cross-chain/quote", post(quote))
        .route("/v1/cross-chain/orders/draft", post(draft_order))
        .route("/v1/cross-chain/orders/direct", post(create_direct_order))
        .route("/v1/cross-chain/orders", post(create_order).get(orders_of))
        .route("/v1/cross-chain/orders/{id}", get(order))
        .route("/v1/cross-chain/orders/{id}/advance", post(advance))
        .with_state(state)
}

/// Build the exact Compact commitment and settlement terms the wallet will authorize.
#[utoipa::path(
    post,
    path = "/v1/cross-chain/orders/draft",
    tag = "cross-chain",
    request_body = DirectOrderDraftRequest,
    responses(
        (status = 200, description = "Wallet-signable direct-order terms", body = DirectOrderDraft),
        (status = 400, description = "Quote or authorization terms are invalid", body = ErrorBody),
        (status = 503, description = "Direct-order drafting is not configured", body = ErrorBody),
    )
)]
async fn draft_order(
    State(state): State<ProxyState>,
    Json(request): Json<DirectOrderDraftRequest>,
) -> Result<Json<DirectOrderDraft>, (StatusCode, Json<ErrorBody>)> {
    let builder = state.direct_drafts.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorBody {
                error: "direct-order drafting is not configured".to_string(),
            }),
        )
    })?;
    builder
        .draft(request, state.clock.now_unix())
        .map(Json)
        .map_err(proxy_error)
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

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateDirectOrderRequest {
    pub draft: DirectOrderDraftRequest,
    #[schema(value_type = String)]
    pub sponsor_signature: Bytes,
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

/// Verify the wallet authorization, obtain chain-authored plans, and start a direct route.
#[utoipa::path(
    post,
    path = "/v1/cross-chain/orders/direct",
    tag = "cross-chain",
    request_body = CreateDirectOrderRequest,
    responses(
        (status = 200, description = "Direct cross-chain order created", body = CrossChainOrderResponse),
        (status = 400, description = "Quote or wallet authorization is invalid", body = ErrorBody),
        (status = 502, description = "A chain service or saga store is unavailable", body = ErrorBody),
        (status = 503, description = "Direct settlement is not configured", body = ErrorBody),
    )
)]
async fn create_direct_order(
    State(state): State<ProxyState>,
    Json(request): Json<CreateDirectOrderRequest>,
) -> Result<Json<CrossChainOrderResponse>, (StatusCode, Json<ErrorBody>)> {
    let builder = state.direct_drafts.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorBody {
                error: "direct-order settlement is not configured".to_string(),
            }),
        )
    })?;
    let now = state.clock.now_unix();
    let authorization = builder
        .authorize(request.draft, request.sponsor_signature, now)
        .map_err(proxy_error)?;
    state
        .proxy
        .start_direct(authorization, now)
        .await
        .map(|order| Json(CrossChainOrderResponse { order }))
        .map_err(proxy_error)
}

/// A person's own cross-chain orders, newest first.
#[utoipa::path(
    get,
    path = "/v1/cross-chain/orders",
    tag = "cross-chain",
    params(
        ("taker" = String, Query, description = "Swapper address"),
        ("limit" = Option<u32>, Query, description = "Page size (default 25, max 200)"),
    ),
    responses(
        (status = 200, description = "The taker's orders", body = CrossChainOrdersResponse),
        (status = 400, description = "Malformed taker address", body = ErrorBody),
        (status = 502, description = "The saga store is unavailable", body = ErrorBody),
    )
)]
async fn orders_of(
    State(state): State<ProxyState>,
    Query(query): Query<OrdersQuery>,
) -> Result<Json<CrossChainOrdersResponse>, (StatusCode, Json<ErrorBody>)> {
    let taker = Address::parse_checksummed(&query.taker, None)
        .or_else(|_| query.taker.parse::<Address>())
        .map_err(|_| client_error("taker must be a 20-byte hex address"))?;
    let limit = query
        .limit
        .unwrap_or(DEFAULT_ORDER_PAGE)
        .min(MAX_ORDER_PAGE);
    state
        .proxy
        .orders_of(taker, limit)
        .await
        .map(|orders| Json(CrossChainOrdersResponse { orders }))
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
        .advance(order_id, state.clock.now_unix())
        .await
        .map(|order| Json(CrossChainOrderResponse { order }))
        .map_err(proxy_error)
}

const DEFAULT_ORDER_PAGE: u32 = 25;
const MAX_ORDER_PAGE: u32 = 200;

#[derive(Debug, Deserialize)]
struct OrdersQuery {
    taker: String,
    limit: Option<u32>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CrossChainOrdersResponse {
    pub orders: Vec<CrossChainSaga>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
struct ErrorBody {
    error: String,
}

fn proxy_error(error: SolventError) -> (StatusCode, Json<ErrorBody>) {
    match error {
        SolventError::InvalidCrossChain(message) => client_error(&message),
        // A chain-local service that refused the request is answering it, not failing: the reason
        // belongs to the person who asked, so it is passed on rather than reported as an outage.
        SolventError::RemoteSolvent(RemoteSolventError::Rejected(message)) => {
            client_error(&message)
        }
        _ => {
            // The reply is deliberately vague; without this the cause reaches nobody at all.
            solvent_core::obs::error!(error = %error, "cross-chain request failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorBody {
                    error: "cross-chain service unavailable".to_string(),
                }),
            )
        }
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
    paths(quote, draft_order, create_direct_order, create_order, order, advance),
    components(schemas(
        CrossChainQuoteRequest,
        CreateCrossChainOrderRequest,
        CreateDirectOrderRequest,
        CrossChainOrderResponse,
        DirectOrderDraftRequest,
        DirectOrderDraft,
        SolventCompactOrder,
        SolventCompactMandate,
        CompactCommitmentTerms,
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
        solvent_core::primitives::crosschain::CrossChainLifecycleStage,
        solvent_core::primitives::crosschain::CrossChainLifecycleEvent,
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
