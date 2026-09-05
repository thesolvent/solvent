//! Inbound HTTP adapter: the response envelope, error mapping, DTOs, and the axum router.
//!
//! Handlers stay thin — they translate HTTP to/from the domain and return [`primitives::ApiResult`];
//! business logic lives in core services.

pub mod dto;
pub mod error;
pub mod handlers;
pub mod openapi;
pub mod primitives;
pub mod state;

use std::time::Duration;

use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::http::state::AppState;

/// Wall-clock ceiling for any request; devnet-generous, tuned per deployment later.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The HTTP router: `/healthz` at root, business routes under `/v1`, all wrapped in the middleware
/// stack (request-id → trace → timeout → CORS).
pub fn router(state: AppState) -> Router {
    let v1 = Router::new()
        .route("/config", get(handlers::config::config))
        .route("/stats", get(handlers::stats::stats))
        .route("/assets", get(handlers::assets::assets))
        .route("/pools", get(handlers::pools::pools))
        .route("/pools/detail", get(handlers::pools::pool_detail))
        .route("/pools/depth", get(handlers::pools::pool_depth))
        .route("/openapi.json", get(openapi::openapi_json))
        .with_state(state);

    Router::new()
        .route("/healthz", get(healthz))
        .nest("/v1", v1)
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
                .layer(TraceLayer::new_for_http())
                .layer(PropagateRequestIdLayer::x_request_id())
                .layer(TimeoutLayer::with_status_code(
                    StatusCode::REQUEST_TIMEOUT,
                    REQUEST_TIMEOUT,
                ))
                .layer(CorsLayer::permissive()),
        )
}

async fn healthz() -> &'static str {
    "ok"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use alloy::primitives::{Address, U256};
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use solvent_core::asset::{AssetManager, TokenList, TokenMeta};
    use solvent_core::deps::ledger::{BudgetSource, BudgetSourceError};
    use solvent_core::ledger::BudgetCache;
    use solvent_core::pool::{DepthService, PoolService};
    use solvent_core::primitives::ledger::AccountKey;
    use solvent_core::registry::SharedSnapshot;
    use tower::ServiceExt;

    use crate::chain::ChainHead;
    use crate::http::state::{AppConfig, Features};

    /// A budget source that funds nothing — enough for the router to wire depth over an empty
    /// registry (the depth tests here exercise routing, not caps).
    struct ZeroBudget;

    #[async_trait::async_trait]
    impl BudgetSource for ZeroBudget {
        async fn budget(&self, _: &AccountKey) -> Result<U256, BudgetSourceError> {
            Ok(U256::ZERO)
        }
    }

    fn test_state() -> AppState {
        let list = TokenList {
            name: "test".to_string(),
            tokens: vec![TokenMeta {
                chain_id: 31337,
                address: Address::from([1; 20]),
                symbol: "WETH".to_string(),
                name: "Wrapped Ether".to_string(),
                decimals: 18,
                logo_uri: None,
                tags: vec![],
            }],
        };
        let registry = Arc::new(SharedSnapshot::default());
        let assets = Arc::new(AssetManager::new(list, Arc::clone(&registry)));
        let pools = Arc::new(PoolService::new(Arc::clone(&registry), Arc::clone(&assets)));
        let depth = Arc::new(DepthService::new(
            Arc::clone(&registry),
            Arc::new(BudgetCache::new(
                Arc::new(ZeroBudget),
                Arc::clone(&registry),
            )),
            Arc::clone(&assets),
        ));
        AppState {
            config: Arc::new(AppConfig {
                chain_id: 31337,
                features: Features {
                    faucet: true,
                    earn: false,
                    send_buy: false,
                },
                default_fee_bps: 5,
                networks: vec!["Ethereum".to_string()],
                block_explorer_url: "http://localhost:5100".to_string(),
            }),
            head: ChainHead::stub(0),
            assets,
            pools,
            depth,
        }
    }

    async fn get(uri: &str) -> (StatusCode, serde_json::Value) {
        let resp = router(test_state())
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn healthz_serves_ok() {
        let resp = router(test_state())
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn config_returns_the_bootstrap_flags() {
        let (status, json) = get("/v1/config").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "Ok");
        assert_eq!(json["result"]["chain_id"], 31337);
        assert_eq!(json["result"]["features"]["earn"], false);
    }

    #[tokio::test]
    async fn assets_returns_the_catalog() {
        let (status, json) = get("/v1/assets").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "Ok");
        assert_eq!(json["result"]["items"][0]["symbol"], "WETH");
        assert_eq!(json["result"]["items"][0]["supported"], false);
    }

    #[tokio::test]
    async fn openapi_doc_is_served() {
        let (status, json) = get("/v1/openapi.json").await;
        assert_eq!(status, StatusCode::OK);
        assert!(json["openapi"].is_string());
        assert!(json["paths"]["/v1/assets"].is_object());
    }

    #[tokio::test]
    async fn pools_serves_a_list() {
        let (status, json) = get("/v1/pools").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "Ok");
        assert!(json["result"]["items"].is_array());
    }

    #[tokio::test]
    async fn pool_detail_unknown_pair_is_404() {
        let (a, b) = (Address::from([1; 20]), Address::from([2; 20]));
        let (status, _) = get(&format!("/v1/pools/detail?base={a}&quote={b}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn pool_detail_malformed_address_is_400() {
        let (status, _) = get("/v1/pools/detail?base=nope&quote=nope").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn pool_depth_unknown_pair_is_404() {
        let (a, b) = (Address::from([1; 20]), Address::from([2; 20]));
        let (status, _) = get(&format!("/v1/pools/depth?base={a}&quote={b}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn pool_depth_malformed_address_is_400() {
        let (status, _) = get("/v1/pools/depth?base=nope&quote=nope&side=sell").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
