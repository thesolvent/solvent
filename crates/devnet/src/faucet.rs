use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use alloy::network::TransactionBuilder;
use alloy::primitives::{Address, U256};
use alloy::providers::{DynProvider, Provider};
use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::cors::CorsLayer;

use crate::cooldown::Cooldown;

sol! {
    #[sol(rpc)]
    interface IDevToken {
        function mint(address to, uint256 amount) external;
    }
}

/// The address manifest written by the devnet deploy script. Only the token table matters to the
/// faucet; the other addresses (aqua/router/…) are ignored.
#[derive(Deserialize)]
pub struct Manifest {
    pub tokens: BTreeMap<String, TokenInfo>,
}

#[derive(Deserialize, Clone)]
pub struct TokenInfo {
    pub address: Address,
    pub decimals: u8,
}

impl Manifest {
    pub fn load(path: &str) -> Result<Self, StartupError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| StartupError::Manifest(format!("{path}: {e}")))?;
        serde_json::from_str(&text).map_err(|e| StartupError::Manifest(format!("{path}: {e}")))
    }

    fn symbols(&self) -> Vec<String> {
        self.tokens.keys().cloned().collect()
    }
}

pub struct AppState {
    provider: DynProvider,
    manifest: Manifest,
    cooldown: Cooldown,
    drip_units: u64,
    gas_target_wei: U256,
}

impl AppState {
    pub fn new(
        provider: DynProvider,
        manifest: Manifest,
        cooldown: Cooldown,
        drip_units: u64,
        gas_target_wei: U256,
    ) -> Self {
        Self {
            provider,
            manifest,
            cooldown,
            drip_units,
            gas_target_wei,
        }
    }

    pub fn token_count(&self) -> usize {
        self.manifest.tokens.len()
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/faucet", post(faucet))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

#[derive(Deserialize)]
struct FaucetRequest {
    address: String,
    /// Which tokens to drip; all of them when omitted.
    #[serde(default)]
    tokens: Option<Vec<String>>,
}

#[derive(Serialize)]
struct Minted {
    symbol: String,
    amount: String,
    tx: String,
}

#[derive(Serialize)]
struct FaucetResponse {
    address: String,
    minted: Vec<Minted>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gas_tx: Option<String>,
}

/// Drip test tokens (and top up gas) to one address, subject to the per-address cooldown.
async fn faucet(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FaucetRequest>,
) -> Result<Json<FaucetResponse>, AppError> {
    let who: Address = req
        .address
        .parse()
        .map_err(|_| AppError::BadRequest(format!("invalid address: {}", req.address)))?;

    if let Err(remaining) = state.cooldown.check(who, Instant::now()) {
        return Err(AppError::Cooldown(remaining.as_secs().max(1)));
    }

    let symbols = req.tokens.unwrap_or_else(|| state.manifest.symbols());
    let mut pending_mints = Vec::with_capacity(symbols.len());
    for symbol in symbols {
        let token = state
            .manifest
            .tokens
            .get(&symbol)
            .ok_or_else(|| AppError::BadRequest(format!("unknown token: {symbol}")))?;
        let amount =
            U256::from(state.drip_units) * U256::from(10u64).pow(U256::from(token.decimals));
        let pending = IDevToken::new(token.address, state.provider.clone())
            .mint(who, amount)
            .send()
            .await
            .map_err(AppError::chain)?;
        pending_mints.push((symbol, amount, pending));
    }

    let mut minted = Vec::with_capacity(pending_mints.len());
    for (symbol, amount, pending) in pending_mints {
        let receipt = pending.get_receipt().await.map_err(AppError::chain)?;
        if !receipt.status() {
            return Err(AppError::Chain(format!(
                "mint {symbol} reverted (tx {:#x})",
                receipt.transaction_hash
            )));
        }
        minted.push(Minted {
            symbol,
            amount: amount.to_string(),
            tx: format!("{:#x}", receipt.transaction_hash),
        });
    }

    let balance = state
        .provider
        .get_balance(who)
        .await
        .map_err(AppError::chain)?;
    let gas_tx = if balance < state.gas_target_wei {
        let request = TransactionRequest::default()
            .with_to(who)
            .with_value(state.gas_target_wei - balance);
        let receipt = state
            .provider
            .send_transaction(request)
            .await
            .map_err(AppError::chain)?
            .get_receipt()
            .await
            .map_err(AppError::chain)?;
        if !receipt.status() {
            return Err(AppError::Chain("gas top-up reverted".to_string()));
        }
        Some(format!("{:#x}", receipt.transaction_hash))
    } else {
        None
    };

    Ok(Json(FaucetResponse {
        address: format!("{who:#x}"),
        minted,
        gas_tx,
    }))
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("cooldown: retry in {0}s")]
    Cooldown(u64),
    #[error("chain error: {0}")]
    Chain(String),
}

impl AppError {
    fn chain<E: std::fmt::Display>(e: E) -> Self {
        AppError::Chain(e.to_string())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Cooldown(_) => StatusCode::TOO_MANY_REQUESTS,
            AppError::Chain(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("config: {0}")]
    Config(String),
    #[error("manifest: {0}")]
    Manifest(String),
    #[error(transparent)]
    Serve(#[from] std::io::Error),
}
