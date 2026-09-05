//! `GET /v1/wallets/{addr}/balances` — the connected wallet's per-token `balance` and `pullable`
//! across the whole catalog. `{addr}` is a single resource id, so it stays a path segment.

use alloy::primitives::Address;
use axum::extract::{Path, State};
use solvent_core::primitives::balances::TokenBalance;
use solvent_core::SolventError;

use crate::http::dto::List;
use crate::http::primitives::{ApiResult, Response};
use crate::http::state::AppState;

/// Balances for `addr` over every catalog token. A malformed address → `400`.
pub async fn balances(
    State(state): State<AppState>,
    Path(addr): Path<String>,
) -> ApiResult<List<TokenBalance>> {
    let owner = addr
        .parse::<Address>()
        .map_err(|e| SolventError::InvalidId {
            id_type: "wallet",
            reason: e.to_string(),
        })?;
    Ok(Response::ok(List::all(
        state.balances.balances(owner).await?,
    )))
}
