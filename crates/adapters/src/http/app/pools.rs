//! `GET /v1/pools` — the pool list, and `GET /v1/pools/detail?base=&quote=` — one pool's detail.
//! A pool is a bidirectional pair, so the list's `token_a`/`token_b` are order-independent asset
//! filters (a pool matches if its pair contains every token given), not a sell/buy direction, and
//! detail's `base`/`quote` identify a pool regardless of order. `type` and `fee` match the pool's
//! classification and popular tier.

use alloy::primitives::Address;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use solvent_core::pool::{Pool, PoolDepth, PoolDetail, PoolType, Side};
use solvent_core::primitives::registry::TokenPair;
use solvent_core::SolventError;

use crate::http::dto::List;
use crate::http::primitives::{ApiResult, Response};
use crate::http::state::AppState;

#[derive(Debug, Deserialize)]
pub struct PoolsQuery {
    #[serde(rename = "type")]
    pool_type: Option<String>,
    token_a: Option<String>,
    token_b: Option<String>,
    fee: Option<String>,
    // No `sort` field yet: the list is always maker-count-desc. apr/tvl/newest sorts need pricing
    // or a created-block read that has no data source, so the field lands with its first backed key.
}

/// List active pools, filtered and ordered by most makers first.
#[utoipa::path(
    get,
    path = "/v1/pools",
    params(
        ("type" = Option<String>, Query, description = "All|Stable|Correlated|Volatile"),
        ("token_a" = Option<String>, Query, description = "Pair must contain this token"),
        ("token_b" = Option<String>, Query, description = "Pair must contain this token"),
        ("fee" = Option<String>, Query, description = "Popular fee tier, e.g. 0.05%"),
    ),
    responses((status = 200, body = Response<List<Pool>>))
)]
pub async fn pools(
    State(state): State<AppState>,
    Query(query): Query<PoolsQuery>,
) -> ApiResult<List<Pool>> {
    let token_a = parse_token(query.token_a.as_deref())?;
    let token_b = parse_token(query.token_b.as_deref())?;

    let mut pools: Vec<Pool> = state
        .pools
        .pools()
        .await
        .into_iter()
        .filter(|pool| matches(pool, &query, token_a, token_b))
        .collect();
    // Most-liquid first; stable tiebreak by pair so the order is deterministic.
    pools.sort_by(|a, b| {
        b.maker_count
            .cmp(&a.maker_count)
            .then_with(|| a.pair.cmp(&b.pair))
    });

    Ok(Response::ok(List::all(pools)))
}

/// The two tokens identifying one pool (order-independent).
#[derive(Debug, Deserialize)]
pub struct PairQuery {
    base: String,
    quote: String,
}

/// Detail for one pool, identified by `base` and `quote`.
#[utoipa::path(
    get,
    path = "/v1/pools/detail",
    params(
        ("base" = String, Query, description = "Base token address"),
        ("quote" = String, Query, description = "Quote token address"),
    ),
    responses(
        (status = 200, body = Response<PoolDetail>),
        (status = 404, description = "No active pool for the pair"),
    )
)]
pub async fn pool_detail(
    State(state): State<AppState>,
    Query(query): Query<PairQuery>,
) -> ApiResult<PoolDetail> {
    let pair = TokenPair::new(parse_addr(&query.base)?, parse_addr(&query.quote)?);
    match state.pools.pool_detail(&pair).await {
        Some(detail) => Ok(Response::ok(detail)),
        None => Err(Response::error("pool not found", StatusCode::NOT_FOUND)),
    }
}

/// The pool plus the depth-curve direction: `side` (defaults to `sell`).
#[derive(Debug, Deserialize)]
pub struct DepthQuery {
    base: String,
    quote: String,
    side: Option<Side>,
}

/// The pair's executable-liquidity depth curve for one side.
#[utoipa::path(
    get,
    path = "/v1/pools/depth",
    params(
        ("base" = String, Query, description = "Base token address"),
        ("quote" = String, Query, description = "Quote token address"),
        ("side" = Option<String>, Query, description = "buy|sell (default sell)"),
    ),
    responses(
        (status = 200, body = Response<PoolDepth>),
        (status = 404, description = "No active pool for the pair"),
    )
)]
pub async fn pool_depth(
    State(state): State<AppState>,
    Query(query): Query<DepthQuery>,
) -> ApiResult<PoolDepth> {
    let pair = TokenPair::new(parse_addr(&query.base)?, parse_addr(&query.quote)?);
    let side = query.side.unwrap_or(Side::Sell);
    match state.depth.depth(&pair, side) {
        Some(depth) => Ok(Response::ok(depth)),
        None => Err(Response::error("pool not found", StatusCode::NOT_FOUND)),
    }
}

fn parse_addr(s: &str) -> Result<Address, SolventError> {
    s.parse::<Address>().map_err(|e| SolventError::InvalidId {
        id_type: "token",
        reason: e.to_string(),
    })
}

fn parse_token(value: Option<&str>) -> Result<Option<Address>, SolventError> {
    value.map(parse_addr).transpose()
}

fn matches(
    pool: &Pool,
    query: &PoolsQuery,
    token_a: Option<Address>,
    token_b: Option<Address>,
) -> bool {
    let type_ok = match query.pool_type.as_deref() {
        None | Some("All") => true,
        Some(t) => type_name(pool.pool_type) == t,
    };
    let fee_ok = match query.fee.as_deref() {
        None | Some("Any") => true,
        Some(f) => pool.popular_fee_tier == f,
    };
    // Symmetric: the pool's pair must contain every token asked for.
    let contains = |token: Option<Address>| {
        token.is_none_or(|a| pool.base.address == a || pool.quote.address == a)
    };
    type_ok && fee_ok && contains(token_a) && contains(token_b)
}

fn type_name(pool_type: PoolType) -> &'static str {
    match pool_type {
        PoolType::Stable => "Stable",
        PoolType::Correlated => "Correlated",
        PoolType::Volatile => "Volatile",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solvent_core::asset::Token;

    fn addr(n: u8) -> Address {
        Address::from([n; 20])
    }

    fn token(n: u8) -> Token {
        Token {
            address: addr(n),
            chain_id: 31337,
            symbol: format!("T{n}"),
            decimals: 18,
        }
    }

    fn pool(a: u8, b: u8, ty: PoolType, fee: &str, makers: u64) -> Pool {
        Pool {
            pair: format!("T{a}/T{b}"),
            base: token(a),
            quote: token(b),
            pool_type: ty,
            maker_count: makers,
            min_spread_bps: 1,
            max_spread_bps: 5,
            popular_fee_tier: fee.to_string(),
            tvl_usd: None,
            volume_24h_usd: None,
            fills_24h: 0,
            apr_pct: None,
        }
    }

    fn query(
        pool_type: Option<&str>,
        a: Option<u8>,
        b: Option<u8>,
        fee: Option<&str>,
    ) -> PoolsQuery {
        PoolsQuery {
            pool_type: pool_type.map(str::to_string),
            token_a: a.map(|n| addr(n).to_string()),
            token_b: b.map(|n| addr(n).to_string()),
            fee: fee.map(str::to_string),
        }
    }

    #[test]
    fn type_filter_selects_matching_pools() {
        let p = pool(1, 2, PoolType::Volatile, "0.05%", 3);
        assert!(matches(
            &p,
            &query(Some("Volatile"), None, None, None),
            None,
            None
        ));
        assert!(!matches(
            &p,
            &query(Some("Stable"), None, None, None),
            None,
            None
        ));
        assert!(matches(
            &p,
            &query(Some("All"), None, None, None),
            None,
            None
        )); // sentinel = no filter
    }

    #[test]
    fn token_filters_are_symmetric_membership() {
        let p = pool(1, 2, PoolType::Volatile, "0.05%", 3);
        // one token → any pool containing it
        assert!(matches(
            &p,
            &query(None, Some(1), None, None),
            Some(addr(1)),
            None
        ));
        assert!(matches(
            &p,
            &query(None, Some(2), None, None),
            Some(addr(2)),
            None
        ));
        // both tokens, either order → the exact pair
        assert!(matches(
            &p,
            &query(None, Some(2), Some(1), None),
            Some(addr(2)),
            Some(addr(1))
        ));
        // a token not in the pair → no match
        assert!(!matches(
            &p,
            &query(None, Some(9), None, None),
            Some(addr(9)),
            None
        ));
    }

    #[test]
    fn malformed_token_is_rejected() {
        assert!(parse_token(Some("not-an-address")).is_err());
        assert!(parse_token(None).unwrap().is_none());
    }
}
