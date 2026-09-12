use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use alloy::primitives::{Address, Bytes, TxHash, U256};
use alloy::providers::{DynProvider, Provider};
use alloy::sol;
use alloy::sol_types::SolCall;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::cors::CorsLayer;
use walletkit::core::deps::SubmissionOpts;
use walletkit::core::wallet::{TxHandle, TxIntent, TxStatus};
use walletkit::Wallet;

use crate::cooldown::Cooldown;

const CONFIRM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

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
    wallet: Arc<Wallet>,
    chain_id: u64,
    manifest: Manifest,
    cooldown: Cooldown,
    drip_units: u64,
    gas_target_wei: U256,
}

impl AppState {
    pub fn new(
        provider: DynProvider,
        wallet: Arc<Wallet>,
        chain_id: u64,
        manifest: Manifest,
        cooldown: Cooldown,
        drip_units: u64,
        gas_target_wei: U256,
    ) -> Self {
        Self {
            provider,
            wallet,
            chain_id,
            manifest,
            cooldown,
            drip_units,
            gas_target_wei,
        }
    }

    pub fn token_count(&self) -> usize {
        self.manifest.tokens.len()
    }

    async fn submit(&self, intent: TxIntent) -> Result<TxHandle, AppError> {
        self.wallet
            .send_with(&intent, SubmissionOpts::public())
            .await
            .map_err(AppError::chain)
    }

    async fn confirm(&self, submitted: TxHandle) -> Result<TxHash, AppError> {
        let deadline = Instant::now() + CONFIRM_TIMEOUT;
        loop {
            let tracked = self
                .wallet
                .handle(submitted.id)
                .await
                .map_err(AppError::chain)?
                .ok_or_else(|| AppError::Chain("faucet transaction disappeared".to_string()))?;
            match tracked.status {
                TxStatus::Confirmed { .. } => {
                    return tracked.broadcasts.last().copied().ok_or_else(|| {
                        AppError::Chain(
                            "confirmed faucet transaction has no broadcast hash".to_string(),
                        )
                    });
                }
                TxStatus::Failed { reason } => return Err(AppError::Chain(reason)),
                TxStatus::Replaced => {
                    return Err(AppError::Chain(
                        "faucet transaction was replaced".to_string(),
                    ));
                }
                TxStatus::Dropped => {
                    return Err(AppError::Chain(
                        "faucet transaction was dropped".to_string(),
                    ));
                }
                _ if Instant::now() >= deadline => {
                    return Err(AppError::Chain(
                        "faucet transaction confirmation timed out".to_string(),
                    ));
                }
                _ => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
            }
        }
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
        let intent = TxIntent::call(
            state.chain_id,
            state.wallet.account(),
            token.address,
            U256::ZERO,
            Bytes::from(IDevToken::mintCall { to: who, amount }.abi_encode()),
        );
        let pending = state.submit(intent).await?;
        pending_mints.push((symbol, amount, pending));
    }

    let mut minted = Vec::with_capacity(pending_mints.len());
    for (symbol, amount, pending) in pending_mints {
        let tx = state.confirm(pending).await?;
        minted.push(Minted {
            symbol,
            amount: amount.to_string(),
            tx: format!("{tx:#x}"),
        });
    }

    let balance = state
        .provider
        .get_balance(who)
        .await
        .map_err(AppError::chain)?;
    let gas_tx = if balance < state.gas_target_wei {
        let intent = TxIntent::transfer(
            state.chain_id,
            state.wallet.account(),
            who,
            state.gas_target_wei - balance,
        );
        Some(format!(
            "{:#x}",
            state.confirm(state.submit(intent).await?).await?
        ))
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
    #[error("wallet state store: {0}")]
    WalletStore(String),
    #[error(transparent)]
    Serve(#[from] std::io::Error),
}
