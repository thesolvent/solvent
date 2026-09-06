//! `POST /v1/swap/quote` — a read-only routing quote: the split across makers, the blended output,
//! and its price impact. No reservation, no persistence (that is `POST /swap`).

use alloy::primitives::{Address, U256};
use axum::extract::{Json, State};
use axum::http::StatusCode;
use serde::Deserialize;
use solvent_core::quote::QuoteResponse;
use solvent_core::SolventError;

use crate::http::primitives::{ApiResult, Response};
use crate::http::state::AppState;

/// A quote request: the pair, and the input size in base units.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct QuoteRequest {
    pub token_in: String,
    pub token_out: String,
    /// The input amount, in base units (a decimal integer string).
    pub amount_in: String,
}

/// Route `amount_in` of `token_in` into `token_out`, returning the split and its impact. `422` when
/// no route exists (no makers, or the size is beyond the book).
#[utoipa::path(
    post,
    path = "/v1/swap/quote",
    request_body = QuoteRequest,
    responses(
        (status = 200, body = Response<QuoteResponse>),
        (status = 422, description = "No route for the pair and size"),
    )
)]
pub async fn quote(
    State(state): State<AppState>,
    Json(body): Json<QuoteRequest>,
) -> ApiResult<QuoteResponse> {
    let token_in = parse_addr(&body.token_in)?;
    let token_out = parse_addr(&body.token_out)?;
    let amount_in = parse_amount(&body.amount_in)?;
    match state.quote.quote(token_in, token_out, amount_in).await {
        Some(quote) => Ok(Response::ok(quote)),
        None => Err(Response::error(
            "no route for this pair and size",
            StatusCode::UNPROCESSABLE_ENTITY,
        )),
    }
}

fn parse_addr(s: &str) -> Result<Address, SolventError> {
    s.parse::<Address>().map_err(|e| SolventError::InvalidId {
        id_type: "token",
        reason: e.to_string(),
    })
}

fn parse_amount(s: &str) -> Result<U256, SolventError> {
    U256::from_str_radix(s, 10).map_err(|e| SolventError::InvalidId {
        id_type: "amount",
        reason: e.to_string(),
    })
}
