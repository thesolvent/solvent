//! `GET /v1/orders` — every order the feed showed us, with the verdict at the door.
//!
//! Distinct from `/v1/trades`, which lists only what this resolver acted on. Most of what arrives
//! never becomes a trade, and without this the refusals are invisible.

use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};

use axum::http::StatusCode;
use solvent_core::SolventError;

use crate::http::primitives::{parse_addr, ApiResult, Response};
use crate::http::state::AppState;
use crate::metrics::{OrderFeedFilter, OrderStateFilter};

/// How many orders to return, how far into the newest-first feed to start, and the header
/// filters: which venue, which pair (directional: what a taker pays for what it wants), and which
/// derived state (the same bucketing the explorer UI computes from verdict plus trade lifecycle).
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OrdersQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub source: Option<String>,
    pub token_in: Option<String>,
    pub token_out: Option<String>,
    pub state: Option<String>,
}

const SOURCES: [&str; 3] = ["uniswapx", "oneinch", "solvent"];

fn parse_filter(query: &OrdersQuery) -> Result<OrderFeedFilter, SolventError> {
    if let Some(source) = &query.source {
        if !SOURCES.contains(&source.as_str()) {
            return Err(SolventError::InvalidId {
                id_type: "order source",
                reason: format!("expected one of {SOURCES:?}, got {source:?}"),
            });
        }
    }
    let state = query
        .state
        .as_deref()
        .map(|s| {
            OrderStateFilter::parse(s).ok_or_else(|| SolventError::InvalidId {
                id_type: "order state",
                reason: format!("unrecognized state {s:?}"),
            })
        })
        .transpose()?;
    Ok(OrderFeedFilter {
        source: query.source.clone(),
        token_in: query
            .token_in
            .as_deref()
            .map(|s| parse_addr("token", s))
            .transpose()?
            .map(|a| a.as_slice().to_vec()),
        token_out: query
            .token_out
            .as_deref()
            .map(|s| parse_addr("token", s))
            .transpose()?
            .map(|a| a.as_slice().to_vec()),
        state,
    })
}

/// One observed order.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ObservedOrder {
    pub order_hash: String,
    /// Where it came from: `uniswapx` (the public book), `oneinch` (the public book), or `solvent`
    /// (our own endpoint).
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
    /// The trade this order became, if it was ever attempted — absent when it was refused at the
    /// door or declined as unprofitable before routing ever reserved anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trade_status: Option<String>,
    /// The trade's own id — what the explorer links to for the trade's full detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trade_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trade_tx_hash: Option<String>,
    /// Why the trade declined — the sim gate's real on-chain revert reason, or the margin call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trade_decline_reason: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ObservedOrders {
    pub items: Vec<ObservedOrder>,
    /// How many orders the feed has ever shown us — the denominator this page sits inside.
    pub total: i64,
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
        return Ok(Response::ok(ObservedOrders {
            items: Vec::new(),
            total: 0,
        }));
    };
    let limit = i64::from(query.limit.unwrap_or(50).min(200));
    let offset = i64::from(query.offset.unwrap_or(0));
    let filter = parse_filter(&query)?;
    // A query failure must not read as "no orders yet" — that is the one answer an operator
    // would act on, and it would be wrong.
    let rows = log
        .recent(limit, offset, &filter)
        .await
        .map_err(|e| Response::error(e, StatusCode::INTERNAL_SERVER_ERROR))?;
    let total = log
        .count(&filter)
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
                trade_status: r.trade_status,
                trade_id: r.trade_id,
                trade_tx_hash: r.trade_tx_hash,
                trade_decline_reason: r.trade_decline_reason,
            })
            .collect(),
        total,
    }))
}
