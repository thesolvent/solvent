//! `GET /v1/makers` — the active-maker roster. `GET /v1/makers/{maker}` — one maker's dashboard.
//! `GET /v1/makers/{maker}/positions` — a maker's positions (list projection). `GET
//! /v1/positions/{hash}` — one position's full detail.

use alloy::primitives::{Address, B256, U256};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use solvent_core::deps::trade::Page as StorePage;
use solvent_core::primitives::maker::{InventoryRow, MakerDashboard, MakerSummary, Position};
use solvent_core::primitives::trade::{Trade as CoreTrade, TradeId, TradeLeg};
use solvent_core::primitives::{MakerId, StrategyHash};
use solvent_core::SolventError;

use crate::http::app::trades::{summary, Trade as TradeDto};
use crate::http::dto::{Cursor, List, Page};
use crate::http::primitives::{ApiResult, Response};
use crate::http::state::AppState;

/// The roster of active makers, each with its position count and shared liquidity.
#[utoipa::path(get, path = "/v1/makers", responses((status = 200, body = Response<List<MakerSummary>>)))]
pub async fn makers(State(state): State<AppState>) -> ApiResult<List<MakerSummary>> {
    Ok(Response::ok(List::all(state.makers.roster().await?)))
}

/// One maker's dashboard (KPIs, fill-share, latency, insight). `{maker}` is an address (`me` needs a
/// wallet).
#[utoipa::path(
    get,
    path = "/v1/makers/{maker}",
    params(("maker" = String, Path, description = "Maker address")),
    responses((status = 200, body = Response<MakerDashboard>))
)]
pub async fn maker_dashboard(
    State(state): State<AppState>,
    Path(maker): Path<String>,
) -> ApiResult<MakerDashboard> {
    let maker = MakerId(parse_maker(&maker)?);
    Ok(Response::ok(state.makers.dashboard(maker).await?))
}

/// A maker's inventory: its positions re-grouped by token (the Assets tab). `{maker}` is an address.
#[utoipa::path(
    get,
    path = "/v1/makers/{maker}/inventory",
    params(("maker" = String, Path, description = "Maker address")),
    responses((status = 200, body = Response<List<InventoryRow>>))
)]
pub async fn maker_inventory(
    State(state): State<AppState>,
    Path(maker): Path<String>,
) -> ApiResult<List<InventoryRow>> {
    let maker = MakerId(parse_maker(&maker)?);
    Ok(Response::ok(List::all(
        state.makers.inventory(maker).await?,
    )))
}

/// One of a maker's settlements: the trade header plus this maker's share and captured fee.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MakerTrade {
    #[serde(flatten)]
    pub trade: TradeDto,
    /// The maker's slice of the trade (delivered ÷ total) — settled trades only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_pct: Option<f64>,
    /// The maker's captured fee, valued at the trade-time prices — settled trades only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_usd: Option<f64>,
}

/// Pagination for the maker settlements feed.
#[derive(Debug, Deserialize)]
pub struct MakerTradesQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}

/// A maker's settlements (newest first, cursor-paginated) — its trades with per-trade share and fee.
#[utoipa::path(
    get,
    path = "/v1/makers/{maker}/trades",
    params(
        ("maker" = String, Path, description = "Maker address"),
        ("limit" = Option<u32>, Query, description = "Page size (default 50, max 200)"),
        ("cursor" = Option<String>, Query, description = "Opaque next-page cursor"),
    ),
    responses((status = 200, body = Response<List<MakerTrade>>))
)]
pub async fn maker_trades(
    State(state): State<AppState>,
    Path(maker): Path<String>,
    Query(query): Query<MakerTradesQuery>,
) -> ApiResult<List<MakerTrade>> {
    let maker = parse_maker(&maker)?;
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
    let fills = state
        .trades
        .list_for_maker(maker, &store_page)
        .await
        .map_err(SolventError::from)?;
    let next_cursor = (fills.len() as u32 == store_page.limit)
        .then(|| {
            fills
                .last()
                .map(|f| Cursor::encode(&f.trade.id.to_string()))
        })
        .flatten();
    let mut items = Vec::with_capacity(fills.len());
    for fill in &fills {
        let header = summary(&state.assets, &state.valuation, &fill.trade).await;
        let dec_in = state.assets.token_or_default(fill.trade.token_in).decimals;
        let dec_out = state.assets.token_or_default(fill.trade.token_out).decimals;
        items.push(MakerTrade {
            share_pct: maker_share_pct(&fill.trade, &fill.leg),
            fee_usd: maker_fee_usd(&fill.trade, &fill.leg, dec_in, dec_out),
            trade: header,
        });
    }
    Ok(Response::ok(List::page(items, next_cursor, None)))
}

/// The maker's slice of the trade — its delivered amount over the trade's total. Settled trades only.
fn maker_share_pct(trade: &CoreTrade, leg: &TradeLeg) -> Option<f64> {
    let total = trade.amount_out?;
    (!total.is_zero()).then(|| to_f64(leg.amount_out) / to_f64(total) * 100.0)
}

/// The maker's captured fee: value received minus value delivered, at the trade-time prices stored on
/// the trade. Settled trades only (an unsettled or unpriced trade has no fee to show).
fn maker_fee_usd(trade: &CoreTrade, leg: &TradeLeg, dec_in: u8, dec_out: u8) -> Option<f64> {
    trade.amount_out?;
    let price_in = trade.token_in_price_usd?;
    let price_out = trade.token_out_price_usd?;
    Some(human(leg.amount_in, dec_in) * price_in - human(leg.amount_out, dec_out) * price_out)
}

/// A same-token ratio numerator/denominator — decimals cancel, so the raw integer converts directly.
fn to_f64(amount: U256) -> f64 {
    amount.to_string().parse().unwrap_or(0.0)
}

/// Base units as a human decimal (for USD math), `0.0` if the decimals are nonsensical.
fn human(amount: U256, decimals: u8) -> f64 {
    alloy::primitives::utils::format_units(amount, decimals)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0)
}

/// A maker's active positions (list projection). `{maker}` is an address (`me` needs a wallet).
#[utoipa::path(
    get,
    path = "/v1/makers/{maker}/positions",
    params(("maker" = String, Path, description = "Maker address")),
    responses((status = 200, body = Response<List<Position>>))
)]
pub async fn maker_positions(
    State(state): State<AppState>,
    Path(maker): Path<String>,
) -> ApiResult<List<Position>> {
    let maker = MakerId(parse_maker(&maker)?);
    Ok(Response::ok(List::all(
        state.makers.positions(maker).await?,
    )))
}

/// One position's full detail, or `404` if the hash is unknown.
#[utoipa::path(
    get,
    path = "/v1/positions/{hash}",
    params(("hash" = String, Path, description = "Strategy hash (position id)")),
    responses(
        (status = 200, body = Response<Position>),
        (status = 404, description = "Unknown position"),
    )
)]
pub async fn position_detail(
    State(state): State<AppState>,
    Path(hash): Path<String>,
) -> ApiResult<Position> {
    let hash = StrategyHash(hash.parse::<B256>().map_err(|e| SolventError::InvalidId {
        id_type: "strategy_hash",
        reason: e.to_string(),
    })?);
    match state.makers.position(hash).await? {
        Some(position) => Ok(Response::ok(position)),
        None => Err(Response::error("position not found", StatusCode::NOT_FOUND)),
    }
}

fn parse_maker(s: &str) -> Result<Address, SolventError> {
    if s == "me" {
        return Err(SolventError::InvalidId {
            id_type: "maker",
            reason: "\"me\" requires a connected wallet".to_string(),
        });
    }
    s.parse::<Address>().map_err(|e| SolventError::InvalidId {
        id_type: "maker",
        reason: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use solvent_core::primitives::trade::TradeStatus;
    use solvent_core::primitives::IntentId;
    use ulid::Ulid;

    fn leg(amount_in: u128, amount_out: u128) -> TradeLeg {
        TradeLeg {
            maker: MakerId(Address::from([3; 20])),
            strategy_hash: StrategyHash(B256::from([3; 32])),
            amount_in: U256::from(amount_in),
            amount_out: U256::from(amount_out),
        }
    }

    fn trade(amount_out: Option<u128>, price_in: Option<f64>, price_out: Option<f64>) -> CoreTrade {
        CoreTrade {
            id: TradeId(Ulid::from_parts(1, 1)),
            order_hash: IntentId(B256::from([1; 32])),
            taker: Address::ZERO,
            token_in: Address::from([2; 20]),
            token_out: Address::from([3; 20]),
            amount_in: U256::from(2_010_000_000u64),
            min_amount_out: U256::ZERO,
            amount_out: amount_out.map(U256::from),
            status: TradeStatus::Confirmed,
            deadline_block: 0,
            signature: None,
            price_impact_pct: None,
            surplus: None,
            tx_hash: None,
            block_number: None,
            created_at: 0,
            settled_at: Some(1),
            token_in_price_usd: price_in,
            token_out_price_usd: price_out,
        }
    }

    #[test]
    fn fee_and_share_use_trade_time_prices() {
        // Maker received 2010 USDC (6dp), delivered 1 WETH (18dp) of a 2-WETH trade.
        let leg = leg(2_010_000_000, 1_000_000_000_000_000_000);
        let t = trade(Some(2_000_000_000_000_000_000), Some(1.0), Some(2000.0));
        // fee = 2010·$1 − 1·$2000 = $10 at the stored prices.
        assert_eq!(maker_fee_usd(&t, &leg, 6, 18), Some(10.0));
        // share = 1 WETH of the 2-WETH trade = 50%.
        assert_eq!(maker_share_pct(&t, &leg), Some(50.0));
    }

    #[test]
    fn unsettled_or_unpriced_has_no_fee() {
        let leg = leg(2_010_000_000, 1_000_000_000_000_000_000);
        // Unsettled (no delivered total): no share, no fee.
        let pending = trade(None, Some(1.0), Some(2000.0));
        assert_eq!(maker_fee_usd(&pending, &leg, 6, 18), None);
        assert_eq!(maker_share_pct(&pending, &leg), None);
        // Settled but unpriced: share available, fee not.
        let unpriced = trade(Some(2_000_000_000_000_000_000), None, None);
        assert_eq!(maker_fee_usd(&unpriced, &leg, 6, 18), None);
        assert_eq!(maker_share_pct(&unpriced, &leg), Some(50.0));
    }
}
