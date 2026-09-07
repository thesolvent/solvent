//! `GET /v1/trades` — the trade list (filtered, cursor-paginated), and `GET /v1/trades/{id}` — one
//! trade's full lifecycle. The list carries only header fields; the detail adds the stage timeline,
//! the maker legs, and the signed order's coordinates. Amounts are rendered against each token's
//! decimals from the catalog.

use alloy::primitives::Address;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use solvent_core::asset::AssetManager;
use solvent_core::deps::trade::{Page as StorePage, TradeFilter};
use solvent_core::primitives::amount::{Amount, TokenAmount};
use solvent_core::primitives::trade::{Trade as CoreTrade, TradeId, TradeInfo};
use solvent_core::valuation::Valuation;
use solvent_core::SolventError;

use crate::http::dto::{Cursor, List, Page};
use crate::http::primitives::{ApiResult, Response};
use crate::http::state::AppState;

/// One trade, header-only in a list; a detail also carries `lifecycle`, `legs`, and the order's
/// coordinates (omitted from JSON when absent).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Trade {
    pub id: String,
    pub status: String,
    #[schema(value_type = String)]
    pub taker: Address,
    /// The swapper's input token and the maximum it authorized.
    pub input: TokenAmount,
    /// The output token and the amount delivered (or the signed floor, until it settles).
    pub output: TokenAmount,
    /// The resolver's surplus over the signed floor, net of gas, in output-token units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surplus: Option<Amount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_number: Option<u64>,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<u64>,
    /// The stage timeline — detail only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<Vec<Action>>,
    /// The maker slices the trade sourced — detail only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legs: Option<Vec<MakerLeg>>,
    /// The signed order hash — detail only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_block: Option<u64>,
}

/// One lifecycle stage a trade reached, and when (unix seconds).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Action {
    pub status: String,
    pub at: u64,
}

/// One maker's slice of the routed split.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MakerLeg {
    pub maker: String,
    pub strategy_hash: String,
    pub amount_in: Amount,
    pub amount_out: Amount,
}

/// List + filter query: pagination (`limit`/`cursor`) and the header filters (`status`, `taker`,
/// and a `base`+`quote` pair, order-independent).
#[derive(Debug, Deserialize)]
pub struct TradesQuery {
    limit: Option<u32>,
    cursor: Option<String>,
    status: Option<String>,
    taker: Option<String>,
    base: Option<String>,
    quote: Option<String>,
}

/// List trades, newest first, filtered and cursor-paginated.
#[utoipa::path(
    get,
    path = "/v1/trades",
    params(
        ("limit" = Option<u32>, Query, description = "Page size (default 50, max 200)"),
        ("cursor" = Option<String>, Query, description = "Opaque next-page cursor"),
        ("status" = Option<String>, Query, description = "Lifecycle status filter"),
        ("taker" = Option<String>, Query, description = "Swapper address filter"),
        ("base" = Option<String>, Query, description = "Pair token (with quote)"),
        ("quote" = Option<String>, Query, description = "Pair token (with base)"),
    ),
    responses((status = 200, body = Response<List<Trade>>))
)]
pub async fn trades(
    State(state): State<AppState>,
    Query(query): Query<TradesQuery>,
) -> ApiResult<List<Trade>> {
    let page = Page {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let cursor = page
        .cursor::<String>()?
        .map(|id| id.parse::<TradeId>())
        .transpose()?;
    let store_page = StorePage {
        limit: page.limit(),
        cursor,
    };
    let filter = TradeFilter {
        status: query.status.as_deref().map(str::parse).transpose()?,
        taker: query.taker.as_deref().map(parse_addr).transpose()?,
        pair: match (query.base.as_deref(), query.quote.as_deref()) {
            (Some(base), Some(quote)) => Some((parse_addr(base)?, parse_addr(quote)?)),
            _ => None,
        },
    };

    let rows = state
        .trades
        .list(&filter, &store_page)
        .await
        .map_err(SolventError::from)?;
    // A full page implies there may be more; the cursor is the last id seen.
    let next_cursor = (rows.len() as u32 == store_page.limit)
        .then(|| rows.last().map(|t| Cursor::encode(&t.id.to_string())))
        .flatten();
    let mut items = Vec::with_capacity(rows.len());
    for trade in &rows {
        items.push(summary(&state.assets, &state.valuation, trade).await);
    }
    Ok(Response::ok(List::page(items, next_cursor, None)))
}

/// One trade's full detail, or `404` if the id is unknown.
#[utoipa::path(
    get,
    path = "/v1/trades/{id}",
    params(("id" = String, Path, description = "Trade id (ULID)")),
    responses(
        (status = 200, body = Response<Trade>),
        (status = 404, description = "Unknown trade"),
    )
)]
pub async fn trade_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Trade> {
    let id = id.parse::<TradeId>()?;
    match state.trades.info(&id).await.map_err(SolventError::from)? {
        Some(info) => Ok(Response::ok(
            detail(&state.assets, &state.valuation, &info).await,
        )),
        None => Err(Response::error("trade not found", StatusCode::NOT_FOUND)),
    }
}

/// A trade's header as the list DTO — heavy fields left empty.
pub(crate) async fn summary(
    assets: &AssetManager,
    valuation: &Valuation,
    trade: &CoreTrade,
) -> Trade {
    let token_in = assets.token_or_default(trade.token_in);
    let token_out = assets.token_or_default(trade.token_out);
    let delivered = trade.amount_out.unwrap_or(trade.min_amount_out);
    let input = valuation
        .amount(trade.amount_in, token_in.address, token_in.decimals)
        .await;
    let output = valuation
        .amount(delivered, token_out.address, token_out.decimals)
        .await;
    let surplus = match trade.surplus {
        Some(s) => Some(
            valuation
                .amount(s, token_out.address, token_out.decimals)
                .await,
        ),
        None => None,
    };
    Trade {
        id: trade.id.to_string(),
        status: trade.status.as_str().to_string(),
        taker: trade.taker,
        input: TokenAmount {
            amount: input,
            token: token_in,
        },
        output: TokenAmount {
            amount: output,
            token: token_out,
        },
        surplus,
        tx_hash: trade.tx_hash.map(|h| h.to_string()),
        block_number: trade.block_number,
        created_at: trade.created_at,
        settled_at: trade.settled_at,
        lifecycle: None,
        legs: None,
        order_hash: None,
        deadline_block: None,
    }
}

/// The full detail DTO — the header plus the stage timeline, maker legs, and order coordinates.
async fn detail(assets: &AssetManager, valuation: &Valuation, info: &TradeInfo) -> Trade {
    let token_in = assets.token_or_default(info.trade.token_in);
    let token_out = assets.token_or_default(info.trade.token_out);
    let mut legs = Vec::with_capacity(info.legs.len());
    for leg in &info.legs {
        legs.push(MakerLeg {
            maker: leg.maker.to_string(),
            strategy_hash: leg.strategy_hash.to_string(),
            amount_in: valuation
                .amount(leg.amount_in, token_in.address, token_in.decimals)
                .await,
            amount_out: valuation
                .amount(leg.amount_out, token_out.address, token_out.decimals)
                .await,
        });
    }
    Trade {
        lifecycle: Some(
            info.attempts
                .iter()
                .map(|a| Action {
                    status: a.status.as_str().to_string(),
                    at: a.at,
                })
                .collect(),
        ),
        legs: Some(legs),
        order_hash: Some(info.trade.order_hash.to_string()),
        deadline_block: Some(info.trade.deadline_block),
        ..summary(assets, valuation, &info.trade).await
    }
}

fn parse_addr(s: &str) -> Result<Address, SolventError> {
    s.parse::<Address>().map_err(|e| SolventError::InvalidId {
        id_type: "token",
        reason: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{B256, U256};
    use async_trait::async_trait;
    use solvent_core::asset::{AssetManager, TokenList, TokenMeta};
    use solvent_core::deps::routing::{PriceOracle, PriceOracleError};
    use solvent_core::primitives::trade::{TradeAttempt, TradeLeg, TradeStatus};
    use solvent_core::primitives::{IntentId, MakerId, StrategyHash, UsdPrice};
    use solvent_core::registry::SharedSnapshot;
    use std::sync::Arc;
    use ulid::Ulid;

    struct NoPrices;
    #[async_trait]
    impl PriceOracle for NoPrices {
        async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError> {
            Err(PriceOracleError::NotFound(token))
        }
    }
    fn valuation() -> Valuation {
        Valuation::new(Arc::new(NoPrices))
    }

    fn assets() -> AssetManager {
        let list = TokenList {
            name: "t".into(),
            tokens: vec![TokenMeta {
                chain_id: 31337,
                address: Address::from([1; 20]),
                symbol: "WETH".into(),
                name: "Wrapped Ether".into(),
                decimals: 18,
                logo_uri: None,
                tags: vec![],
            }],
        };
        AssetManager::new(list, Arc::new(SharedSnapshot::default()))
    }

    fn a_trade() -> CoreTrade {
        CoreTrade {
            id: TradeId(Ulid::from_parts(1, 2)),
            order_hash: IntentId(B256::from([7; 32])),
            taker: Address::from([9; 20]),
            token_in: Address::from([1; 20]),
            token_out: Address::from([2; 20]),
            amount_in: U256::from(1_000u64),
            min_amount_out: U256::from(500u64),
            amount_out: None,
            status: TradeStatus::Reserved,
            deadline_block: 123,
            signature: None,
            price_impact_pct: None,
            surplus: Some(U256::from(3u64)),
            tx_hash: None,
            block_number: None,
            created_at: 100,
            settled_at: None,
            token_in_price_usd: None,
            token_out_price_usd: None,
        }
    }

    #[tokio::test]
    async fn list_summary_omits_the_heavy_fields() {
        let dto = summary(&assets(), &valuation(), &a_trade()).await;
        assert!(dto.lifecycle.is_none());
        assert!(dto.legs.is_none());
        assert!(dto.order_hash.is_none());
        assert!(dto.deadline_block.is_none());
        // A catalog token renders its decimals; the unfilled output shows the signed floor.
        assert_eq!(dto.input.token.symbol, "WETH");
        assert_eq!(dto.output.amount.raw, "500");
    }

    #[tokio::test]
    async fn detail_includes_lifecycle_legs_and_order() {
        let info = TradeInfo {
            trade: a_trade(),
            attempts: vec![TradeAttempt {
                status: TradeStatus::Reserved,
                at: 100,
            }],
            legs: vec![TradeLeg {
                maker: MakerId(Address::from([5; 20])),
                strategy_hash: StrategyHash(B256::from([6; 32])),
                amount_in: U256::from(1_000u64),
                amount_out: U256::from(500u64),
            }],
        };
        let dto = detail(&assets(), &valuation(), &info).await;
        assert_eq!(dto.lifecycle.as_deref().unwrap().len(), 1);
        assert_eq!(dto.legs.as_deref().unwrap().len(), 1);
        assert_eq!(dto.order_hash.unwrap(), info.trade.order_hash.to_string());
        assert_eq!(dto.deadline_block, Some(123));
    }
}
