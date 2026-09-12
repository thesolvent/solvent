//! `GET /v1/orders` — every order the feed showed us, with the verdict at the door.
//!
//! Distinct from `/v1/trades`, which lists only what this resolver acted on. Most of what arrives
//! never becomes a trade, and without this the refusals are invisible.

use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};

use axum::http::StatusCode;

use crate::http::primitives::{ApiResult, Response};
use crate::http::state::AppState;

/// How many orders to return; the feed is newest-first.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OrdersQuery {
    pub limit: Option<u32>,
}

/// One observed order.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ObservedOrder {
    pub order_hash: String,
    /// Where it came from: `uniswapx` (the public book) or `solvent` (our own endpoint).
    pub source: String,
    pub token_in: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_out: Option<String>,
    pub amount_in: String,
    /// What the settler demands, priced when the order was seen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_out: Option<String>,
    /// `admitted` or `dropped`.
    pub verdict: String,
    /// The admission rule that refused it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What sourcing the delivery would cost us, once routing priced it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indicative_in: Option<String>,
    pub seen_at: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ObservedOrders {
    pub items: Vec<ObservedOrder>,
}

#[utoipa::path(
    get, path = "/v1/orders", params(OrdersQuery), tag = "explorer",
    responses((status = 200, body = ObservedOrders)),
)]
pub async fn orders(
    State(state): State<AppState>,
    Query(query): Query<OrdersQuery>,
) -> ApiResult<ObservedOrders> {
    let Some(log) = state.order_log.as_ref() else {
        return Ok(Response::ok(ObservedOrders { items: Vec::new() }));
    };
    let limit = i64::from(query.limit.unwrap_or(50).min(200));
    // A query failure must not read as "no orders yet" — that is the one answer an operator
    // would act on, and it would be wrong.
    let rows = log
        .recent(limit)
        .await
        .map_err(|e| Response::error(e, StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(Response::ok(ObservedOrders {
        items: rows
            .into_iter()
            .map(|r| ObservedOrder {
                order_hash: r.order_hash,
                source: r.source,
                token_in: r.token_in,
                token_out: r.token_out,
                amount_in: r.amount_in,
                required_out: r.required_out,
                verdict: r.verdict,
                reason: r.reason,
                indicative_in: r.indicative_in,
                seen_at: r.seen_at,
            })
            .collect(),
    }))
}
