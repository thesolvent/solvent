//! The read-only public UniswapX simulation feed.

use alloy::primitives::Address;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use solvent_core::obs::error;
use solvent_core::primitives::IntentId;

use crate::http::dto::{Cursor, List, Page};
use crate::http::primitives::{ApiResult, Response};
use crate::http::state::AppState;
use crate::ingest::uniswapx::{UniswapFeedAsset, UniswapXFeedCursor, UniswapXFeedOrder};

type ViewResult<T> = Result<T, Response<()>>;

#[derive(Debug, Deserialize)]
pub struct UniswapXFeedQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[non_exhaustive]
pub struct UniswapXFeedAssetView {
    #[schema(value_type = String)]
    pub address: Address,
    pub symbol: String,
    pub logo_uri: Option<String>,
    pub decimals: u8,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[non_exhaustive]
pub struct UniswapXFeedOrderView {
    pub order_hash: String,
    pub source_chain_id: u64,
    pub token_in: UniswapXFeedAssetView,
    pub token_out: UniswapXFeedAssetView,
    pub amount_in: String,
    pub required_out: String,
    pub market_out_per_in_q18: String,
    pub simulated_amount_out: String,
    pub simulated_batch_id: u64,
    pub observed_at: u64,
    pub last_seen_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UniswapXFeedCursorView {
    last_seen_at: u64,
    order_hash: IntentId,
}

#[utoipa::path(
    get,
    path = "/v1/uniswapx-feed",
    params(
        ("limit" = Option<u32>, Query, description = "Page size (default 50, max 200)"),
        ("cursor" = Option<String>, Query, description = "Opaque next-page cursor"),
    ),
    responses((status = 200, body = Response<List<UniswapXFeedOrderView>>))
)]
pub async fn uniswapx_feed(
    State(state): State<AppState>,
    Query(query): Query<UniswapXFeedQuery>,
) -> ApiResult<List<UniswapXFeedOrderView>> {
    let page = Page {
        limit: query.limit,
        cursor: query.cursor,
    };
    let cursor = page.cursor::<UniswapXFeedCursorView>()?;
    let Some(store) = &state.uniswap_feed else {
        return Ok(Response::ok(List::all(Vec::new())));
    };
    let mut orders = store
        .list(
            cursor.map(|cursor| UniswapXFeedCursor {
                last_seen_at: cursor.last_seen_at,
                order_hash: cursor.order_hash,
            }),
            page.limit().saturating_add(1),
        )
        .await
        .map_err(|store_error| {
            let _ = &store_error;
            error!(error = %store_error, "UniswapX feed query failed");
            Response::error(
                "could not load UniswapX feed",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
    let has_more = orders.len() > page.limit() as usize;
    orders.truncate(page.limit() as usize);
    let next_cursor = has_more.then(|| orders.last()).flatten().map(|order| {
        Cursor::encode(&UniswapXFeedCursorView {
            last_seen_at: order.last_seen_at,
            order_hash: order.order_hash,
        })
    });
    let views = orders
        .into_iter()
        .map(|order| view(order, &state.uniswap_feed_assets))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Response::ok(List::page(views, next_cursor, None)))
}

fn view(
    order: UniswapXFeedOrder,
    assets: &std::collections::HashMap<Address, UniswapFeedAsset>,
) -> ViewResult<UniswapXFeedOrderView> {
    let token_in = asset(order.token_in, assets)?;
    let token_out = asset(order.token_out, assets)?;
    Ok(UniswapXFeedOrderView {
        order_hash: order.order_hash.to_string(),
        source_chain_id: order.source_chain_id,
        token_in,
        token_out,
        amount_in: order.amount_in.to_string(),
        required_out: order.required_out.to_string(),
        market_out_per_in_q18: order.market_out_per_in_q18.to_string(),
        simulated_amount_out: order.simulated_amount_out.to_string(),
        simulated_batch_id: order.simulated_batch_id,
        observed_at: order.observed_at,
        last_seen_at: order.last_seen_at,
    })
}

fn asset(
    address: Address,
    assets: &std::collections::HashMap<Address, UniswapFeedAsset>,
) -> ViewResult<UniswapXFeedAssetView> {
    let Some(asset) = assets.get(&address) else {
        return Err(Response::error(
            "UniswapX feed asset configuration is incomplete",
            StatusCode::INTERNAL_SERVER_ERROR,
        ));
    };
    Ok(UniswapXFeedAssetView {
        address,
        symbol: asset.symbol.clone(),
        logo_uri: asset.logo_uri.clone(),
        decimals: asset.decimals,
    })
}
