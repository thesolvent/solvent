//! `POST /v1/positions/preview` — server-authoritative pre-flight for a ship the SDK already
//! encoded: whether the strategy exists, which tokens still need an approval, and any warnings.

use alloy::primitives::{Address, B256, U256};
use axum::extract::{Json, State};
use serde::Deserialize;
use solvent_core::primitives::maker::PreviewResponse;
use solvent_core::primitives::StrategyHash;
use solvent_core::SolventError;

use crate::http::primitives::{parse_addr, ApiResult, Response};
use crate::http::state::AppState;

/// The SDK-encoded strategy and the amounts the maker intends to ship, keyed to a wallet.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PreviewRequest {
    pub maker: String,
    pub strategy_hash: String,
    pub amounts: Vec<AmountIn>,
}

/// One leg of a ship: a token and the amount in base units (a decimal integer string).
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AmountIn {
    pub token: String,
    pub amount: String,
}

#[utoipa::path(
    post,
    path = "/v1/positions/preview",
    request_body = PreviewRequest,
    responses((status = 200, body = Response<PreviewResponse>))
)]
pub async fn preview(
    State(state): State<AppState>,
    Json(body): Json<PreviewRequest>,
) -> ApiResult<PreviewResponse> {
    let maker = parse_addr("maker", &body.maker)?;
    let hash = StrategyHash(parse_hash(&body.strategy_hash)?);
    let amounts = body
        .amounts
        .iter()
        .map(|a| Ok((parse_addr("token", &a.token)?, parse_amount(&a.amount)?)))
        .collect::<Result<Vec<(Address, U256)>, SolventError>>()?;
    Ok(Response::ok(
        state.makers.preview(maker, hash, &amounts).await?,
    ))
}

fn parse_hash(s: &str) -> Result<B256, SolventError> {
    s.parse::<B256>().map_err(|e| SolventError::InvalidId {
        id_type: "strategy_hash",
        reason: e.to_string(),
    })
}

fn parse_amount(s: &str) -> Result<U256, SolventError> {
    U256::from_str_radix(s, 10).map_err(|e| SolventError::InvalidId {
        id_type: "amount",
        reason: e.to_string(),
    })
}
