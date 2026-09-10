//! Read-only public work queue for signed, inventory-backed rebate executions.

use alloy::primitives::Address;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Serialize;
use solvent_core::primitives::rebate::RebateExecution;
use solvent_core::primitives::RebateBatchId;

use crate::http::primitives::{ApiResult, Response};
use crate::http::state::AppState;

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RebateWork {
    pub id: String,
    #[schema(value_type = String)]
    pub to: Address,
    #[schema(value_type = String)]
    pub maker: Address,
    pub strategy_hash: String,
    #[schema(value_type = String)]
    pub token_in: Address,
    #[schema(value_type = String)]
    pub token_out: Address,
    pub amount_in: String,
    pub amount_out: String,
    pub maker_rebate: String,
    pub executor_profit: String,
    pub nonce: String,
    pub deadline_block: u64,
    pub calldata: String,
    pub published_at: u64,
}

#[utoipa::path(
    get,
    path = "/v1/rebates",
    responses((status = 200, body = Response<Vec<RebateWork>>))
)]
pub async fn rebates(State(state): State<AppState>) -> ApiResult<Vec<RebateWork>> {
    Ok(Response::ok(
        state
            .rebates
            .ready()
            .await
            .into_iter()
            .map(|execution| view(execution, state.config.filler))
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/v1/rebates/{id}",
    params(("id" = String, Path, description = "Rebate batch id")),
    responses(
        (status = 200, body = Response<RebateWork>),
        (status = 404, description = "Unknown or non-executable rebate batch"),
    )
)]
pub async fn rebate_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<RebateWork> {
    let id = id.parse::<RebateBatchId>()?;
    match state.rebates.ready_by_id(id).await {
        Some(execution) => Ok(Response::ok(view(execution, state.config.filler))),
        None => Err(Response::error("rebate not found", StatusCode::NOT_FOUND)),
    }
}

fn view(execution: RebateExecution, target: Address) -> RebateWork {
    let plan = execution.plan.as_ref();
    RebateWork {
        id: plan.batch_id.to_string(),
        to: target,
        maker: plan.strategy.maker.0,
        strategy_hash: plan.strategy.strategy_hash.to_string(),
        token_in: plan.token_in,
        token_out: plan.token_out,
        amount_in: plan.amount_in.to_string(),
        amount_out: plan.amount_out.to_string(),
        maker_rebate: plan.maker_rebate.to_string(),
        executor_profit: plan.executor_profit.to_string(),
        nonce: execution.authorization.nonce.to_string(),
        deadline_block: execution.authorization.deadline_block,
        calldata: format!("0x{}", alloy::hex::encode(&execution.calldata)),
        published_at: execution.published_at,
    }
}
