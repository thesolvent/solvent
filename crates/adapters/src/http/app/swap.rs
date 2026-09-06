//! `POST /v1/swap/quote` — a read-only routing quote. `POST /v1/swap` — submit a taker-signed order
//! (UniswapX Orders API shape): decode, verify the swapper signature, cosign, then route → reserve →
//! persist → fill via the core `SwapService`.

use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, U256};
use axum::extract::{Json, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use solvent_core::deps::ingest::Normalizer;
use solvent_core::deps::quote_log::{QuoteParticipant, QuoteServed};
use solvent_core::primitives::quote::QuoteLeg;
use solvent_core::primitives::registry::TokenPair;
use solvent_core::primitives::trade::TradeId;
use solvent_core::primitives::{ChainId, MakerId, StrategyHash};
use solvent_core::quote::QuoteResponse;
use solvent_core::SolventError;
use std::time::Instant;
use ulid::Ulid;

use crate::http::primitives::{ApiResult, Response};
use crate::http::state::AppState;
use crate::ingest::uniswapx::UniswapXV2Normalizer;

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

    let started = Instant::now();
    let result = state.quote.quote(token_in, token_out, amount_in).await;
    let served = QuoteServed {
        chain_id: ChainId(state.config.chain_id),
        pair: TokenPair::new(token_in, token_out),
        latency_ms: started.elapsed().as_millis() as u64,
        participants: result
            .as_ref()
            .map(|q| q.legs.iter().filter_map(participant).collect())
            .unwrap_or_default(),
    };
    // Best-effort analytics: a log failure must never fail the quote.
    if let Err(err) = state.quote_log.record(&served).await {
        tracing::warn!(error = %err, "quote log record failed");
    }

    match result {
        Some(quote) => Ok(Response::ok(quote)),
        None => Err(Response::error(
            "no route for this pair and size",
            StatusCode::UNPROCESSABLE_ENTITY,
        )),
    }
}

/// The maker + strategy a quote leg sourced, or `None` if its hash doesn't parse.
fn participant(leg: &QuoteLeg) -> Option<QuoteParticipant> {
    Some(QuoteParticipant {
        maker: MakerId(leg.maker),
        strategy_hash: StrategyHash(leg.strategy_hash.parse().ok()?),
    })
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

/// A taker-signed order submission, mirroring the UniswapX Orders API (`{ encodedOrder, signature,
/// chainId, quoteId? }`). The taker's client builds + signs the base order; the server cosigns.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SwapRequest {
    #[serde(rename = "encodedOrder")]
    pub encoded_order: String,
    pub signature: String,
    #[serde(rename = "chainId")]
    pub chain_id: u64,
    #[serde(rename = "quoteId", default)]
    pub quote_id: Option<String>,
}

/// The created trade and where its lifecycle stands (`created`…`submitted`, or `declined`).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SwapResponse {
    pub trade_id: String,
    pub status: String,
}

/// Submit a taker-signed order: decode → verify the swapper signature → cosign → route → reserve →
/// persist → fill. A malformed or unverifiable order is `400`; an unroutable one returns a
/// `declined` trade (`200`).
#[utoipa::path(
    post,
    path = "/v1/swap",
    request_body = SwapRequest,
    responses(
        (status = 200, body = Response<SwapResponse>),
        (status = 400, description = "Malformed or unverifiable order"),
    )
)]
pub async fn submit(
    State(state): State<AppState>,
    Json(body): Json<SwapRequest>,
) -> ApiResult<SwapResponse> {
    let encoded = hex_bytes("encodedOrder", &body.encoded_order)?;
    let signature = hex_bytes("signature", &body.signature)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let cosigned = state
        .cosigner
        .cosign(&encoded, signature.into(), now)
        .map_err(|e| Response::error(e.to_string(), StatusCode::BAD_REQUEST))?;
    let intent = UniswapXV2Normalizer
        .normalize(&cosigned.raw)
        .map_err(SolventError::from)?;
    let outcome = state
        .swap
        .submit(intent, cosigned.swapper, TradeId(Ulid::new()))
        .await?;
    Ok(Response::ok(SwapResponse {
        trade_id: outcome.trade_id.to_string(),
        status: outcome.status.as_str().to_string(),
    }))
}

fn hex_bytes(field: &'static str, s: &str) -> Result<Vec<u8>, SolventError> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    alloy::hex::decode(s).map_err(|e| SolventError::InvalidId {
        id_type: field,
        reason: e.to_string(),
    })
}
