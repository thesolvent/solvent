//! Inbound HTTP adapter: the response envelope, error mapping, DTOs, and the axum router.
//!
//! Handlers stay thin — they translate HTTP to/from the domain and return [`primitives::ApiResult`];
//! business logic lives in core services.

pub mod app;
pub mod dto;
pub mod error;
pub mod openapi;
pub mod primitives;
pub mod state;

use std::time::Duration;

use axum::http::StatusCode;
use axum::routing::{get, post};
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
        .route("/config", get(app::config::config))
        .route("/stats", get(app::stats::stats))
        .route("/assets", get(app::assets::assets))
        .route("/pools", get(app::pools::pools))
        .route("/pools/detail", get(app::pools::pool_detail))
        .route("/pools/depth", get(app::pools::pool_depth))
        .route("/swap/quote", post(app::swap::quote))
        .route("/wallets/{addr}/balances", get(app::balances::balances))
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

    use std::collections::BTreeMap;

    use alloy::primitives::{Address, U256};
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use solvent_core::asset::{AssetManager, TokenList, TokenMeta};
    use solvent_core::balances::BalancesService;
    use solvent_core::balances::Holdings;
    use solvent_core::deps::balances::{BalancesOracle, BalancesOracleError};
    use solvent_core::deps::ledger::{
        BudgetSource, BudgetSourceError, LedgerStore, LedgerStoreError,
    };
    use solvent_core::deps::routing::{GasPrice, PriceOracle};
    use solvent_core::ledger::LedgerService;
    use solvent_core::pool::{DepthService, PoolService};
    use solvent_core::primitives::ledger::{AccountKey, Reservation};
    use solvent_core::primitives::routing::RoutingConfig;
    use solvent_core::primitives::ReservationId;
    use solvent_core::quote::QuoteService;
    use solvent_core::registry::SharedSnapshot;
    use tower::ServiceExt;

    use crate::chain::ChainHead;
    use crate::http::state::{AppConfig, Features};
    use crate::ledger::SystemClock;
    use crate::routing::MarketCache;

    /// A budget source that funds nothing — enough for the router to wire depth over an empty
    /// registry (the depth tests here exercise routing, not caps).
    struct ZeroBudget;

    #[async_trait::async_trait]
    impl BudgetSource for ZeroBudget {
        async fn budget(&self, _: &AccountKey) -> Result<U256, BudgetSourceError> {
            Ok(U256::ZERO)
        }
    }

    /// A no-op ledger store — these tests exercise routing over an empty registry, never reserving,
    /// so the ledger only needs to construct.
    struct NoopLedgerStore;

    #[async_trait::async_trait]
    impl LedgerStore for NoopLedgerStore {
        async fn reserve(&self, _: &Reservation) -> Result<(), LedgerStoreError> {
            Ok(())
        }
        async fn post(&self, _: ReservationId, _: &[U256]) -> Result<(), LedgerStoreError> {
            Ok(())
        }
        async fn void(&self, _: ReservationId) -> Result<(), LedgerStoreError> {
            Ok(())
        }
        async fn expire(&self, _: ReservationId) -> Result<(), LedgerStoreError> {
            Ok(())
        }
        async fn void_reorg(&self, _: ReservationId) -> Result<(), LedgerStoreError> {
            Ok(())
        }
        async fn open_reservations(&self) -> Result<Vec<Reservation>, LedgerStoreError> {
            Ok(Vec::new())
        }
    }

    /// A balances oracle that reports nothing — the wallet endpoint then lists every catalog token
    /// at zero, which is what the balances tests here assert.
    struct ZeroOracle;

    #[async_trait::async_trait]
    impl BalancesOracle for ZeroOracle {
        async fn holdings(
            &self,
            _: Address,
            _: &[Address],
        ) -> Result<BTreeMap<Address, Holdings>, BalancesOracleError> {
            Ok(BTreeMap::new())
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
        let ledger = Arc::new(LedgerService::new(
            Arc::new(NoopLedgerStore),
            Arc::new(ZeroBudget),
            Arc::new(SystemClock),
        ));
        let depth = Arc::new(DepthService::new(
            Arc::clone(&registry),
            Arc::clone(&ledger),
            Arc::clone(&assets),
        ));
        let market = MarketCache::new();
        let gas: Arc<dyn GasPrice> = market.clone();
        let oracle: Arc<dyn PriceOracle> = market;
        let quote = Arc::new(QuoteService::new(
            Arc::clone(&registry),
            Arc::clone(&ledger),
            Arc::clone(&assets),
            RoutingConfig::new(16, 4, 0),
            Arc::new(SystemClock),
            gas,
            oracle,
            Address::ZERO,
        ));
        let balances = Arc::new(BalancesService::new(
            Arc::new(ZeroOracle),
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
            balances,
            quote,
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

    async fn post(uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let resp = router(test_state())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
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
        // Every M1 read path and a representative nested schema are in the document.
        for path in [
            "/v1/assets",
            "/v1/pools",
            "/v1/pools/detail",
            "/v1/pools/depth",
            "/v1/swap/quote",
            "/v1/wallets/{addr}/balances",
        ] {
            assert!(json["paths"][path].is_object(), "missing path {path}");
        }
        assert!(json["components"]["schemas"]["PoolDetail"].is_object());
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

    #[tokio::test]
    async fn wallet_balances_list_the_whole_catalog() {
        let addr = Address::from([7; 20]);
        let (status, json) = get(&format!("/v1/wallets/{addr}/balances")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "Ok");
        assert_eq!(json["result"]["items"][0]["token"]["symbol"], "WETH");
        assert_eq!(json["result"]["items"][0]["balance"]["display"], "0");
    }

    #[tokio::test]
    async fn wallet_balances_malformed_addr_is_400() {
        let (status, _) = get("/v1/wallets/nope/balances").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn swap_quote_no_route_is_422() {
        // The test registry is empty, so any pair is unroutable.
        let (a, b) = (Address::from([1; 20]), Address::from([2; 20]));
        let (status, json) = post(
            "/v1/swap/quote",
            serde_json::json!({
                "token_in": a.to_string(),
                "token_out": b.to_string(),
                "amount_in": "1000",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["status"], "Error");
    }

    #[tokio::test]
    async fn swap_quote_malformed_address_is_400() {
        let (status, _) = post(
            "/v1/swap/quote",
            serde_json::json!({ "token_in": "nope", "token_out": "nope", "amount_in": "1000" }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
