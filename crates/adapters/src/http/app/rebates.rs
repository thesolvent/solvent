//! Public executable rebate queue and durable execution history.

use alloy::primitives::Address;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use solvent_core::deps::rebate::{ExecutedRebateCursor, ExecutedRebateQuery};
use solvent_core::primitives::rebate::{RebateAllocation, RebateExecution, RebateSettlement};
use solvent_core::primitives::{MakerId, RebateBatchId, SolventError};

use crate::http::dto::{Cursor, List, Page};
use crate::http::primitives::{parse_addr, ApiResult, Response};
use crate::http::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RebateStatus {
    Ready,
    Executed,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[non_exhaustive]
pub struct RebateAllocationView {
    pub trade_id: String,
    pub amount: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[non_exhaustive]
pub struct RebateWork {
    pub id: String,
    pub status: RebateStatus,
    #[schema(value_type = String)]
    pub maker: Address,
    pub strategy_hash: String,
    #[schema(value_type = String)]
    pub token_in: Address,
    #[schema(value_type = String)]
    pub token_out: Address,
    pub amount_in: String,
    pub amount_out: String,
    pub gross_surplus: String,
    pub safe_gas_cost: String,
    pub maker_rebate: String,
    pub executor_profit: String,
    pub deviation_bps: u64,
    pub allocations: Vec<RebateAllocationView>,
    #[schema(value_type = Option<String>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<Address>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_block: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calldata: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<u64>,
    #[schema(value_type = Option<String>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executor: Option<Address>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executed_at: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct RebatesQuery {
    limit: Option<u32>,
    cursor: Option<String>,
    status: Option<String>,
    maker: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RebateCursor {
    recorded_at: u64,
    id: String,
}

struct RebateRecord {
    id: RebateBatchId,
    recorded_at: u64,
    view: RebateWork,
}

#[utoipa::path(
    get,
    path = "/v1/rebates",
    params(
        ("limit" = Option<u32>, Query, description = "Page size (default 50, max 200)"),
        ("cursor" = Option<String>, Query, description = "Opaque next-page cursor"),
        ("status" = Option<String>, Query, description = "ready or executed"),
        ("maker" = Option<String>, Query, description = "Maker address filter"),
    ),
    responses((status = 200, body = Response<List<RebateWork>>))
)]
pub async fn rebates(
    State(state): State<AppState>,
    Query(query): Query<RebatesQuery>,
) -> ApiResult<List<RebateWork>> {
    let page = Page {
        limit: query.limit,
        cursor: query.cursor,
    };
    let cursor = page
        .cursor::<RebateCursor>()?
        .map(RebateCursor::into_domain)
        .transpose()?;
    let status = query.status.as_deref().map(parse_status).transpose()?;
    let maker = query
        .maker
        .as_deref()
        .map(|maker| parse_addr("maker", maker).map(MakerId))
        .transpose()?;
    let fetch_limit = page.limit().saturating_add(1);
    let current_head = state.head.latest();
    let mut records = Vec::new();

    if status != Some(RebateStatus::Executed) {
        records.extend(
            state
                .rebates
                .ready()
                .await
                .into_iter()
                .filter(|execution| execution.authorization.deadline_block > current_head)
                .filter(|execution| {
                    maker.is_none_or(|maker| execution.plan.strategy.maker == maker)
                })
                .map(|execution| ready_record(execution, state.config.filler))
                .filter(|record| cursor.is_none_or(|cursor| record.is_before(cursor))),
        );
    }
    if status != Some(RebateStatus::Ready) {
        let history = state
            .rebates
            .executed(&ExecutedRebateQuery::new(maker, cursor, fetch_limit))
            .await?;
        records.extend(history.into_iter().map(executed_record));
    }

    records.sort_unstable_by(|left, right| {
        right
            .recorded_at
            .cmp(&left.recorded_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    let has_more = records.len() > page.limit() as usize;
    records.truncate(page.limit() as usize);
    let next_cursor = has_more
        .then(|| records.last().map(RebateRecord::cursor))
        .flatten()
        .map(|cursor| Cursor::encode(&cursor));
    Ok(Response::ok(List::page(
        records.into_iter().map(|record| record.view).collect(),
        next_cursor,
        None,
    )))
}

#[utoipa::path(
    get,
    path = "/v1/rebates/{id}",
    params(("id" = String, Path, description = "Rebate batch id")),
    responses(
        (status = 200, body = Response<RebateWork>),
        (status = 404, description = "Unknown rebate batch"),
    )
)]
pub async fn rebate_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<RebateWork> {
    let id = id.parse::<RebateBatchId>()?;
    if let Some(execution) = state.rebates.ready_by_id(id).await {
        if execution.authorization.deadline_block > state.head.latest() {
            return Ok(Response::ok(
                ready_record(execution, state.config.filler).view,
            ));
        }
    }
    match state.rebates.executed_by_id(id).await? {
        Some(settlement) => Ok(Response::ok(executed_record(settlement).view)),
        None => Err(Response::error("rebate not found", StatusCode::NOT_FOUND)),
    }
}

impl RebateCursor {
    fn into_domain(self) -> Result<ExecutedRebateCursor, SolventError> {
        Ok(ExecutedRebateCursor::new(
            self.recorded_at,
            self.id.parse()?,
        ))
    }
}

impl RebateRecord {
    fn is_before(&self, cursor: ExecutedRebateCursor) -> bool {
        self.recorded_at < cursor.executed_at
            || (self.recorded_at == cursor.executed_at && self.id < cursor.batch_id)
    }

    fn cursor(&self) -> RebateCursor {
        RebateCursor {
            recorded_at: self.recorded_at,
            id: self.id.to_string(),
        }
    }
}

fn ready_record(execution: RebateExecution, target: Address) -> RebateRecord {
    let plan = execution.plan.as_ref();
    RebateRecord {
        id: plan.batch_id,
        recorded_at: execution.published_at,
        view: RebateWork {
            id: plan.batch_id.to_string(),
            status: RebateStatus::Ready,
            maker: plan.strategy.maker.0,
            strategy_hash: plan.strategy.strategy_hash.to_string(),
            token_in: plan.token_in,
            token_out: plan.token_out,
            amount_in: plan.amount_in.to_string(),
            amount_out: plan.amount_out.to_string(),
            gross_surplus: plan.gross_surplus.to_string(),
            safe_gas_cost: plan.safe_gas_cost.to_string(),
            maker_rebate: plan.maker_rebate.to_string(),
            executor_profit: plan.executor_profit.to_string(),
            deviation_bps: plan.deviation_bps,
            allocations: allocations(&plan.allocations),
            to: Some(target),
            nonce: Some(execution.authorization.nonce.to_string()),
            deadline_block: Some(execution.authorization.deadline_block),
            calldata: Some(format!("0x{}", alloy::hex::encode(&execution.calldata))),
            published_at: Some(execution.published_at),
            executor: None,
            transaction_hash: None,
            block_number: None,
            executed_at: None,
        },
    }
}

fn executed_record(settlement: RebateSettlement) -> RebateRecord {
    let plan = settlement.plan.as_ref();
    RebateRecord {
        id: plan.batch_id,
        recorded_at: settlement.executed_at,
        view: RebateWork {
            id: plan.batch_id.to_string(),
            status: RebateStatus::Executed,
            maker: plan.strategy.maker.0,
            strategy_hash: plan.strategy.strategy_hash.to_string(),
            token_in: plan.token_in,
            token_out: plan.token_out,
            amount_in: plan.amount_in.to_string(),
            amount_out: plan.amount_out.to_string(),
            gross_surplus: plan.gross_surplus.to_string(),
            safe_gas_cost: plan.safe_gas_cost.to_string(),
            maker_rebate: plan.maker_rebate.to_string(),
            executor_profit: plan.executor_profit.to_string(),
            deviation_bps: plan.deviation_bps,
            allocations: allocations(&plan.allocations),
            to: None,
            nonce: None,
            deadline_block: None,
            calldata: None,
            published_at: None,
            executor: Some(settlement.event.executor),
            transaction_hash: Some(settlement.event.tx_hash.to_string()),
            block_number: Some(settlement.event.block_number),
            executed_at: Some(settlement.executed_at),
        },
    }
}

fn allocations(values: &[RebateAllocation]) -> Vec<RebateAllocationView> {
    values
        .iter()
        .map(|allocation| RebateAllocationView {
            trade_id: allocation.trade_id.to_string(),
            amount: allocation.amount.to_string(),
        })
        .collect()
}

fn parse_status(value: &str) -> Result<RebateStatus, SolventError> {
    match value {
        "ready" => Ok(RebateStatus::Ready),
        "executed" => Ok(RebateStatus::Executed),
        _ => Err(SolventError::InvalidId {
            id_type: "rebate status",
            reason: "expected ready or executed".to_string(),
        }),
    }
}
