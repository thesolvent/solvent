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

#[derive(Clone)]
struct ProxyState {
    proxy: Arc<CrossChainProxy>,
    clock: Arc<dyn Clock>,
}

pub fn crosschain_proxy_router(proxy: Arc<CrossChainProxy>, clock: Arc<dyn Clock>) -> Router {
    let state = ProxyState { proxy, clock };
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/openapi.json", get(openapi))
        .route("/v1/cross-chain/quote", post(quote))
        .route("/v1/cross-chain/orders", post(create_order))
        .route("/v1/cross-chain/orders/{id}", get(order))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
pub struct CrossChainQuoteRequest {
    pub request_id: B256,
    pub origin_chain_id: u64,
    pub destination_chain_id: u64,
    pub origin_token_in: Address,
    pub origin_token_out: Address,
    pub destination_token_in: Address,
    pub destination_token_out: Address,
    pub amount_in: U256,
    pub destination_amount_in: U256,
    pub deadline_unix: u64,
    pub route: CrossChainRoute,
}

#[derive(Debug, Deserialize)]
pub struct CreateCrossChainOrderRequest {
    pub order_id: CrossChainOrderId,
    pub quote: AggregateQuote,
    pub origin_plan: ChainExecutionPlan,
    pub destination_plan: ChainExecutionPlan,
}

#[derive(Debug, Serialize)]
pub struct CrossChainOrderResponse {
    pub order: CrossChainSaga,
}

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

#[derive(Debug, Serialize)]
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

async fn openapi() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "openapi": "3.1.0",
        "info": { "title": "Solvent Cross-Chain Proxy API", "version": "0.1.0" },
        "paths": {
            "/v1/cross-chain/quote": { "post": { "summary": "Join origin and destination Solvent quotes" } },
            "/v1/cross-chain/orders": { "post": { "summary": "Prepare and start a cross-chain order" } },
            "/v1/cross-chain/orders/{id}": { "get": { "summary": "Read durable cross-chain order state" } }
        }
    }))
}
