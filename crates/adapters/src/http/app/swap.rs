//! `POST /v1/swap/quote` — a read-only routing quote. `POST /v1/swap` — authenticate a supported
//! taker-signed order, then route → reserve → persist → fill via the core `SwapService`.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, Bytes, U256};
use axum::extract::{Json, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use solvent_core::deps::ingest::Normalizer;
use solvent_core::deps::quote_log::{QuoteParticipant, QuoteServed};
use solvent_core::primitives::ingest::{ProtocolId, RawOrder};
use solvent_core::primitives::quote::QuoteLeg;
use solvent_core::primitives::registry::TokenPair;
use solvent_core::primitives::trade::TradeId;
use solvent_core::primitives::{ChainId, MakerId, StrategyHash};
use solvent_core::quote::{QuoteOutcome, QuoteResponse};
use solvent_core::swap::TradePrices;
use solvent_core::SolventError;
use ulid::Ulid;

use crate::http::primitives::{parse_addr, ApiResult, Response};
use crate::http::state::AppState;
use crate::ingest::uniswapx::UniswapXV2Normalizer;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SwapProtocol {
    #[default]
    Uniswapx,
    Erc7683,
}

/// A quote request: the pair, and the input size in base units.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct QuoteRequest {
    #[serde(default)]
    pub protocol: SwapProtocol,
    pub token_in: String,
    pub token_out: String,
    /// The input amount, in base units (a decimal integer string).
    pub amount_in: String,
    /// The permitted price movement, in basis points. When present, the quote is also checked
    /// against the exact order bound used at submission.
    pub slippage_bps: Option<u16>,
}

/// Route `amount_in` of `token_in` into `token_out`, returning the split and its impact. `422` when
/// no route exists or the submitted order could not cover estimated settlement costs.
#[utoipa::path(
    post,
    path = "/v1/swap/quote",
    request_body = QuoteRequest,
    responses(
        (status = 200, body = Response<QuoteResponse>),
        (status = 422, description = "No route for the pair and size, or order cannot cover estimated settlement costs"),
    )
)]
pub async fn quote(
    State(state): State<AppState>,
    Json(body): Json<QuoteRequest>,
) -> ApiResult<QuoteResponse> {
    let token_in = parse_addr("token", &body.token_in)?;
    let token_out = parse_addr("token", &body.token_out)?;
    let amount_in = parse_amount(&body.amount_in)?;
    if token_in == Address::ZERO || token_out == Address::ZERO {
        return Err(SolventError::InvalidId {
            id_type: "swap token",
            reason: "the zero address is not supported".to_string(),
        }
        .into());
    }
    if token_in == token_out {
        return Err(SolventError::InvalidId {
            id_type: "swap pair",
            reason: "input and output tokens must be distinct".to_string(),
        }
        .into());
    }
    if amount_in.is_zero() {
        return Err(SolventError::InvalidId {
            id_type: "amount",
            reason: "the input amount must be positive".to_string(),
        }
        .into());
    }
    if body
        .slippage_bps
        .is_some_and(|slippage_bps| slippage_bps >= 10_000)
    {
        return Err(Response::error(
            "slippage_bps must be between 0 and 9,999",
            StatusCode::BAD_REQUEST,
        ));
    }

    let (routing_input, executor_fee) = match body.protocol {
        SwapProtocol::Uniswapx => (amount_in, None),
        SwapProtocol::Erc7683 => {
            let policy = state.erc7683_fee_policy.ok_or_else(erc7683_unavailable)?;
            (
                policy.routing_input(amount_in)?,
                Some(policy.fee(amount_in)?),
            )
        }
    };

    let started = Instant::now();
    let mut outcome = match body.slippage_bps {
        Some(slippage_bps) => {
            state
                .quote
                .quote_for_order(token_in, token_out, routing_input, slippage_bps)
                .await
        }
        None => match state.quote.quote(token_in, token_out, routing_input).await {
            Some(quote) => QuoteOutcome::Quote(Box::new(quote)),
            None => QuoteOutcome::NoRoute,
        },
    };
    if let (QuoteOutcome::Quote(quote), Some(fee)) = (&mut outcome, executor_fee) {
        quote.executor_fee = Some(
            state
                .valuation
                .amount(fee, token_in, state.assets.decimals(&token_in))
                .await,
        );
    }
    let served = QuoteServed {
        chain_id: ChainId(state.config.chain_id),
        pair: TokenPair::new(token_in, token_out),
        latency_ms: started.elapsed().as_millis() as u64,
        participants: match &outcome {
            QuoteOutcome::Quote(quote) => quote.legs.iter().filter_map(participant).collect(),
            _ => Vec::new(),
        },
    };
    // Best-effort analytics: a log failure must never fail the quote.
    if let Err(err) = state.quote_log.record(&served).await {
        tracing::warn!(error = %err, "quote log record failed");
    }

    match outcome {
        QuoteOutcome::Quote(quote) => Ok(Response::ok(*quote)),
        QuoteOutcome::NoRoute => Err(Response::error(
            "no route for this pair and size",
            StatusCode::UNPROCESSABLE_ENTITY,
        )),
        QuoteOutcome::SettlementCostExceedsLimit => Err(Response::error(
            "this amount cannot cover estimated settlement costs at the selected slippage; increase the amount or raise the slippage limit",
            StatusCode::UNPROCESSABLE_ENTITY,
        )),
        _ => Err(Response::error(
            "could not price this trade",
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

fn parse_amount(s: &str) -> Result<U256, SolventError> {
    U256::from_str_radix(s, 10).map_err(|e| SolventError::InvalidId {
        id_type: "amount",
        reason: e.to_string(),
    })
}

/// A taker-signed order submission. UniswapX remains the default when `protocol` is omitted.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct SwapRequest {
    #[serde(default)]
    pub protocol: SwapProtocol,
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

/// Submit a taker-signed order: authenticate → route → reserve → persist → fill. A malformed or
/// unverifiable order is `400`; an unroutable one returns a `declined` trade (`200`).
#[utoipa::path(
    post,
    path = "/v1/swap",
    request_body = SwapRequest,
    responses(
        (status = 200, body = Response<SwapResponse>),
        (status = 400, description = "Malformed or unverifiable order"),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn submit(
    State(state): State<AppState>,
    Json(body): Json<SwapRequest>,
) -> ApiResult<SwapResponse> {
    if body.chain_id != state.config.chain_id {
        return Err(Response::error(
            "order chainId does not match this deployment",
            StatusCode::BAD_REQUEST,
        ));
    }
    let encoded = hex_bytes("encodedOrder", &body.encoded_order)?;
    let signature = hex_bytes("signature", &body.signature)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (intent, taker) = match body.protocol {
        SwapProtocol::Uniswapx => {
            let cosigned = state
                .cosigner
                .cosign(&encoded, signature.into(), now)
                .map_err(|e| Response::error(e.to_string(), StatusCode::BAD_REQUEST))?;
            let intent = UniswapXV2Normalizer
                .normalize(&cosigned.raw)
                .map_err(SolventError::from)?;
            (intent, cosigned.swapper)
        }
        SwapProtocol::Erc7683 => {
            let raw = RawOrder::new(
                ProtocolId::Erc7683,
                ChainId(body.chain_id),
                Bytes::from(encoded),
                Bytes::from(signature),
                now,
            );
            let normalizer = state.erc7683.as_ref().ok_or_else(erc7683_unavailable)?;
            let normalized = normalizer
                .normalize_order(&raw)
                .map_err(SolventError::from)?;
            (normalized.intent, normalized.user)
        }
    };
    // Capture trade-time token prices here (the adapter holds the oracle) so the trade's fee/value
    // figures stay fixed at submit rather than drifting with the market.
    let prices = TradePrices {
        token_in_usd: state
            .valuation
            .price(intent.input.token)
            .await
            .map(|p| p.to_f64()),
        token_out_usd: match intent.outputs.first() {
            Some(output) => state
                .valuation
                .price(output.token)
                .await
                .map(|p| p.to_f64()),
            None => None,
        },
    };
    let outcome = state
        .swap
        .submit(intent, taker, TradeId(Ulid::new()), prices)
        .await?;
    Ok(Response::ok(SwapResponse {
        trade_id: outcome.trade_id.to_string(),
        status: outcome.status.as_str().to_string(),
    }))
}

fn erc7683_unavailable() -> SolventError {
    SolventError::InvalidId {
        id_type: "swap protocol",
        reason: "ERC-7683 is not configured on this deployment".to_string(),
    }
}

fn hex_bytes(field: &'static str, s: &str) -> Result<Vec<u8>, SolventError> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    alloy::hex::decode(s).map_err(|e| SolventError::InvalidId {
        id_type: field,
        reason: e.to_string(),
    })
}
