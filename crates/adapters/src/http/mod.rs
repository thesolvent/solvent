//! Inbound HTTP adapter: the response envelope, error mapping, DTOs, and the axum router.
//!
//! Handlers stay thin — they translate HTTP to/from the domain and return [`primitives::ApiResult`];
//! business logic lives in core services.

pub mod app;
mod depth;
pub mod dto;
pub mod error;
pub mod openapi;
pub mod primitives;
pub mod state;

pub use depth::DepthReader;

use std::time::Duration;

use axum::extract::State;
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
        .route("/swap", post(app::swap::submit))
        .route("/orders", get(app::orders::orders))
        .route("/trades", get(app::trades::trades))
        .route("/trades/{id}", get(app::trades::trade_detail))
        .route("/activity", get(app::activity::activity))
        .route("/makers", get(app::makers::makers))
        .route("/makers/{maker}", get(app::makers::maker_dashboard))
        .route(
            "/makers/{maker}/inventory",
            get(app::makers::maker_inventory),
        )
        .route("/makers/{maker}/trades", get(app::makers::maker_trades))
        .route(
            "/makers/{maker}/positions",
            get(app::makers::maker_positions),
        )
        .route("/pairs", get(app::pairs::pairs))
        .route("/pairs/history", get(app::pairs::pair_history))
        .route("/positions/preview", post(app::positions::preview))
        .route("/positions/{hash}", get(app::makers::position_detail))
        .route("/positions/{hash}/depth", get(app::makers::position_depth))
        .route(
            "/positions/{hash}/history",
            get(app::makers::position_history),
        )
        .route("/wallets/{addr}/balances", get(app::balances::balances))
        .route("/openapi.json", get(openapi::openapi_json))
        .with_state(state.clone());

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(state)
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

/// Readiness, distinct from liveness: the process is fine, but a resolver that cannot see orders
/// cannot do its job. `503` here should drain traffic and page someone; it should not restart the
/// process, since the usual cause is upstream and a restart will not fix it.
async fn readyz(State(state): State<AppState>) -> (StatusCode, &'static str) {
    let stale = state
        .feed_health
        .as_ref()
        .is_some_and(|health| !health.is_live(now_unix()));
    match stale {
        true => (StatusCode::SERVICE_UNAVAILABLE, "order feed unreachable"),
        false => (StatusCode::OK, "ready"),
    }
}

/// Wall-clock seconds. The feed stamps its successes from the same source.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use std::collections::BTreeMap;

    use alloy::primitives::{Address, Bytes, B256, U256};
    use alloy::signers::local::PrivateKeySigner;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use solvent_core::asset::{
        AssetManager, PairHistoryService, PairPriceHistory, PriceHistoryPeriod, TokenList,
        TokenMeta,
    };
    use solvent_core::balances::BalancesService;
    use solvent_core::balances::Holdings;
    use solvent_core::deps::asset::{PairPriceHistorySource, PairPriceHistorySourceError};
    use solvent_core::deps::balances::{BalancesOracle, BalancesOracleError};
    use solvent_core::deps::execution::{
        Execution, ExecutionError, SettlementError, SettlementReader, SimError, SimGate,
    };
    use solvent_core::deps::ingest::{FillBuilder, FillBuilderError};
    use solvent_core::deps::ledger::{
        BudgetSource, BudgetSourceError, LedgerStore, LedgerStoreError,
    };
    use solvent_core::deps::maker_metrics::{
        MakerMetrics, MakerMetricsError, MakerMetricsStore, PairMetrics, PositionMetrics,
    };
    use solvent_core::deps::quote_log::{QuoteLog, QuoteLogError, QuoteServed};
    use solvent_core::deps::registry::{EventStore, RecordedEvent, StoreError};
    use solvent_core::deps::routing::{GasPrice, PriceOracle};
    use solvent_core::deps::trade::{
        CreateResult, MakerFill, Page, Settlement, TradeFilter, TradeStats, TradeStore,
        TradeStoreError,
    };
    use solvent_core::execution::ExecutionService;
    use solvent_core::ledger::LedgerService;
    use solvent_core::maker::MakerService;
    use solvent_core::pool::{DepthService, PoolService};
    use solvent_core::primitives::execution::{
        ExecHandle, ExecStatus, FillTx, SimVerdict, TrackedFill,
    };
    use solvent_core::primitives::ingest::{Intent, ProtocolId};
    use solvent_core::primitives::ledger::{AccountKey, Reservation, ReservationSource};
    use solvent_core::primitives::registry::TokenPair;
    use solvent_core::primitives::registry::{AquaEvent, EventCursor, EventExt, Snapshot};
    use solvent_core::primitives::routing::{RoutePlan, RoutingConfig};
    use solvent_core::primitives::trade::{
        Trade, TradeAttempt, TradeId, TradeInfo, TradeLeg, TradeStatus,
    };
    use solvent_core::primitives::{ChainId, IntentId, ReservationId};
    use solvent_core::primitives::{MakerId, StrategyHash};
    use solvent_core::quote::QuoteService;
    use solvent_core::registry::SharedSnapshot;
    use solvent_core::routing::LegCostResolver;
    use solvent_core::swap::{SwapConfig, SwapService};
    use solvent_core::trade::TradeService;
    use solvent_core::valuation::Valuation;
    use tower::ServiceExt;

    use crate::chain::ChainHead;
    use crate::http::state::{AppConfig, Features};
    use crate::ingest::uniswapx::{
        FeedHealth, ServerCosigner, UniswapXV2Normalizer as ServerNormalizer,
    };
    use crate::ledger::SystemClock;
    use crate::routing::MarketCache;

    /// A budget source that funds nothing — enough for the router to wire depth over an empty
    /// registry (the depth tests here exercise routing, not caps).
    struct ZeroBudget;

    struct EmptyPairHistory;

    #[async_trait::async_trait]
    impl PairPriceHistorySource for EmptyPairHistory {
        async fn history(
            &self,
            base: Address,
            quote: Address,
            period: PriceHistoryPeriod,
        ) -> Result<PairPriceHistory, PairPriceHistorySourceError> {
            Ok(PairPriceHistory {
                base,
                quote,
                period,
                points: Vec::new(),
            })
        }
    }

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

    /// No-op swap-path fakes: the handler tests reach `400` before the service is called, so these
    /// only need to construct.
    struct NoopTrades;
    #[async_trait::async_trait]
    impl TradeStore for NoopTrades {
        async fn create(
            &self,
            trade: &Trade,
            _: &[TradeLeg],
            _: &[TradeAttempt],
        ) -> Result<CreateResult, TradeStoreError> {
            Ok(CreateResult {
                id: trade.id,
                created: true,
            })
        }
        async fn advance(
            &self,
            _: &TradeId,
            _: TradeStatus,
            _: u64,
        ) -> Result<(), TradeStoreError> {
            Ok(())
        }
        async fn settle(&self, _: &TradeId, _: &Settlement) -> Result<(), TradeStoreError> {
            Ok(())
        }
        async fn info(&self, _: &TradeId) -> Result<Option<TradeInfo>, TradeStoreError> {
            Ok(None)
        }
        async fn find_by_order(&self, _: &IntentId) -> Result<Option<Trade>, TradeStoreError> {
            Ok(None)
        }
        async fn list(&self, _: &TradeFilter, _: &Page) -> Result<Vec<Trade>, TradeStoreError> {
            Ok(Vec::new())
        }
        async fn list_for_maker(
            &self,
            _: Address,
            _: &Page,
            _: Option<std::ops::Range<u64>>,
        ) -> Result<Vec<MakerFill>, TradeStoreError> {
            Ok(Vec::new())
        }
        async fn stats(&self) -> Result<TradeStats, TradeStoreError> {
            Ok(TradeStats {
                settled: 0,
                confirmed: 0,
                failed: 0,
                median_impact_pct: None,
            })
        }
    }

    /// An empty registry event store — the activity/stats endpoints read nothing in these tests.
    struct NoopEventStore;
    #[async_trait::async_trait]
    impl solvent_core::deps::registry::BlockTimes for NoopEventStore {
        async fn timestamps(
            &self,
            _: &[B256],
        ) -> Result<
            std::collections::BTreeMap<B256, u64>,
            solvent_core::deps::registry::BlockTimesError,
        > {
            Ok(Default::default())
        }
    }
    #[async_trait::async_trait]
    impl EventStore for NoopEventStore {
        async fn cursor(&self, _: ChainId) -> Result<Option<EventCursor>, StoreError> {
            Ok(None)
        }
        async fn insert(
            &self,
            _: ChainId,
            _: &[EventExt<AquaEvent>],
        ) -> Result<Vec<EventExt<AquaEvent>>, StoreError> {
            Ok(Vec::new())
        }
        async fn save_cursor(&self, _: ChainId, _: EventCursor) -> Result<(), StoreError> {
            Ok(())
        }
        async fn events(&self, _: ChainId) -> Result<Vec<EventExt<AquaEvent>>, StoreError> {
            Ok(Vec::new())
        }
        async fn history(
            &self,
            _: ChainId,
            _: StrategyHash,
        ) -> Result<Vec<EventExt<AquaEvent>>, StoreError> {
            Ok(Vec::new())
        }
        async fn recent(
            &self,
            _: ChainId,
            _: Option<EventCursor>,
            _: u32,
        ) -> Result<Vec<RecordedEvent>, StoreError> {
            Ok(Vec::new())
        }
        async fn count_since(&self, _: ChainId, _: u64) -> Result<u64, StoreError> {
            Ok(0)
        }
    }

    struct NoopQuoteLog;
    #[async_trait::async_trait]
    impl QuoteLog for NoopQuoteLog {
        async fn record(&self, _: &QuoteServed) -> Result<(), QuoteLogError> {
            Ok(())
        }
    }

    struct NoopMakerMetrics;
    #[async_trait::async_trait]
    impl MakerMetricsStore for NoopMakerMetrics {
        async fn maker(
            &self,
            _: MakerId,
            _: std::ops::Range<u64>,
        ) -> Result<MakerMetrics, MakerMetricsError> {
            Ok(MakerMetrics {
                fills: 0,
                activity: Vec::new(),
                last_fill_at: None,
                volume: Vec::new(),
                inflow: Vec::new(),
                quotes: 0,
                latency_p50_ms: None,
            })
        }
        async fn position(
            &self,
            _: StrategyHash,
            _: TokenPair,
            _: std::ops::Range<u64>,
        ) -> Result<PositionMetrics, MakerMetricsError> {
            Ok(PositionMetrics {
                daily_fills: vec![0; 7],
                fills: 0,
                volume: Vec::new(),
                last_fill_at: None,
                quote_uptime_pct: None,
            })
        }
        async fn pair_activity(
            &self,
            _: TokenPair,
            _: u64,
        ) -> Result<PairMetrics, MakerMetricsError> {
            Ok(PairMetrics {
                fills: 0,
                volume: vec![],
            })
        }

        async fn pair_fills(
            &self,
            _: &[TokenPair],
            _: std::ops::Range<u64>,
        ) -> Result<u64, MakerMetricsError> {
            Ok(0)
        }
    }

    struct FakeSim;
    #[async_trait::async_trait]
    impl SimGate for FakeSim {
        async fn simulate(&self, _: &FillTx) -> Result<SimVerdict, SimError> {
            Ok(SimVerdict::Ok)
        }
    }
    struct FakeExec;
    #[async_trait::async_trait]
    impl Execution for FakeExec {
        async fn submit(
            &self,
            fill: &FillTx,
            _: ReservationId,
        ) -> Result<ExecHandle, ExecutionError> {
            Ok(ExecHandle(fill.intent.0))
        }
        async fn status(&self, _: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError> {
            Ok(Some(ExecStatus::Pending))
        }
        async fn forget(&self, _: IntentId) -> Result<(), ExecutionError> {
            Ok(())
        }
        async fn tracked(&self) -> Result<Vec<TrackedFill>, ExecutionError> {
            Ok(Vec::new())
        }
        async fn tick(&self) -> Result<(), ExecutionError> {
            Ok(())
        }
    }
    struct FakeSettle;
    #[async_trait::async_trait]
    impl SettlementReader for FakeSettle {
        async fn settled(
            &self,
            _: B256,
            _: &[ReservationSource],
        ) -> Result<Vec<U256>, SettlementError> {
            Ok(Vec::new())
        }
    }
    struct FakeFill;
    impl FillBuilder for FakeFill {
        fn build(
            &self,
            _: &Intent,
            _: &RoutePlan,
            _: &Snapshot,
        ) -> Result<Bytes, FillBuilderError> {
            Ok(Bytes::new())
        }
    }

    pub(super) fn test_state() -> AppState {
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
        let ledger = Arc::new(LedgerService::new(
            Arc::new(NoopLedgerStore),
            Arc::new(ZeroBudget),
            Arc::new(SystemClock),
        ));
        let market = MarketCache::new();
        let gas: Arc<dyn GasPrice> = market.clone();
        let oracle: Arc<dyn PriceOracle> = market;
        let valuation = Arc::new(Valuation::new(Arc::clone(&oracle)));
        let leg_cost = Arc::new(LegCostResolver::new(
            gas,
            oracle,
            Arc::clone(&assets),
            Address::ZERO,
            0,
        ));
        let pools = Arc::new(PoolService::new(
            Arc::clone(&registry),
            Arc::clone(&assets),
            Arc::clone(&valuation),
            Arc::new(NoopMakerMetrics),
            Arc::new(ZeroOracle),
            Arc::new(SystemClock),
        ));
        let depth = Arc::new(DepthService::new(
            Arc::clone(&registry),
            Arc::clone(&ledger),
            Arc::clone(&assets),
        ));
        let quote = Arc::new(QuoteService::new(
            Arc::clone(&registry),
            Arc::clone(&ledger),
            Arc::clone(&assets),
            RoutingConfig::new(16, 4, 0),
            Arc::new(SystemClock),
            Arc::clone(&leg_cost),
            Arc::clone(&valuation),
        ));
        let execution = Arc::new(ExecutionService::new(
            Arc::new(FakeSim),
            Arc::new(FakeExec),
            Arc::new(FakeSettle),
            Arc::clone(&ledger),
        ));
        let trades: Arc<dyn TradeStore> = Arc::new(NoopTrades);
        let swap = Arc::new(SwapService::new(
            Arc::clone(&registry),
            Arc::clone(&ledger),
            Arc::clone(&trades),
            execution,
            BTreeMap::from([(
                ProtocolId::UniswapXV2,
                Arc::new(FakeFill) as Arc<dyn FillBuilder>,
            )]),
            Arc::clone(&leg_cost),
            Arc::new(SystemClock),
            SwapConfig {
                routing: RoutingConfig::new(16, 4, 0),
                chain_id: 31337,
                filler: Address::ZERO,
                filler_owner: Address::ZERO,
                reservation_ttl_secs: 60,
            },
        ));
        let cosigner = Arc::new(ServerCosigner::new(
            Address::ZERO,
            Address::ZERO,
            31337,
            PrivateKeySigner::from_bytes(&B256::from([1u8; 32])).unwrap(),
            Address::ZERO,
            60,
        ));
        let balances = Arc::new(BalancesService::new(
            Arc::new(ZeroOracle),
            Arc::clone(&assets),
            Arc::clone(&valuation),
        ));
        let makers = Arc::new(MakerService::new(
            Arc::clone(&registry),
            Arc::clone(&assets),
            Arc::clone(&valuation),
            Arc::new(NoopMakerMetrics),
            Arc::new(ZeroOracle),
            Arc::new(NoopEventStore),
            Arc::new(NoopEventStore),
            Arc::new(SystemClock),
            ChainId(31337),
        ));
        let trade_svc = Arc::new(TradeService::new(
            Arc::clone(&trades),
            Arc::clone(&assets),
            Arc::clone(&valuation),
            Arc::clone(&registry),
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
                aqua: Address::ZERO,
                app: Address::ZERO,
                reactor: Address::ZERO,
                permit2: Address::ZERO,
                cosigner: Address::ZERO,
            }),
            head: ChainHead::stub(0),
            assets,
            pair_history: Arc::new(PairHistoryService::new(Arc::new(EmptyPairHistory))),
            pools,
            depth: DepthReader::new(depth),
            balances,
            makers,
            quote,
            swap,
            cosigner,
            normalizer: Arc::new(ServerNormalizer::new(Address::ZERO, vec![Address::ZERO])),
            trades: trade_svc,
            registry: Arc::clone(&registry),
            registry_store: Arc::new(NoopEventStore),
            valuation,
            quote_log: Arc::new(NoopQuoteLog),
            feed_health: None,
            order_log: None,
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

    /// Readiness reflects the feed: a resolver that cannot reach the Orders API is not ready, even
    /// though the process is perfectly alive. Without this the only signal is a constant "ok".
    #[tokio::test]
    async fn readyz_fails_when_the_order_feed_is_stale() {
        let health = Arc::new(FeedHealth::new(Duration::from_secs(60)));
        let mut state = test_state();
        state.feed_health = Some(Arc::clone(&health));
        let app = router(state);

        let code = |app: Router| async move {
            app.oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response")
            .status()
        };
        assert_eq!(
            code(app.clone()).await,
            StatusCode::OK,
            "nothing attempted yet"
        );

        // One failed attempt, long enough ago to exhaust the silence budget.
        let long_ago = now_unix().saturating_sub(3_600);
        health.record_failure(long_ago, false);
        assert_eq!(
            code(app).await,
            StatusCode::SERVICE_UNAVAILABLE,
            "the feed has been unreachable past its budget"
        );
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
        assert_eq!(
            json["result"]["aqua"],
            "0x0000000000000000000000000000000000000000"
        );
        assert_eq!(
            json["result"]["app"],
            "0x0000000000000000000000000000000000000000"
        );
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
    async fn pair_history_returns_the_requested_orientation_and_period() {
        let base = Address::from([1; 20]);
        let quote = Address::from([2; 20]);
        let uri = format!("/v1/pairs/history?base={base}&quote={quote}&period=7d");

        let (status, json) = get(&uri).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["result"]["base"], base.to_string());
        assert_eq!(json["result"]["quote"], quote.to_string());
        assert_eq!(json["result"]["period"], "7d");
        assert_eq!(json["result"]["points"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn openapi_doc_is_served() {
        let (status, json) = get("/v1/openapi.json").await;
        assert_eq!(status, StatusCode::OK);
        assert!(json["openapi"].is_string());
        // Every read path and a representative nested schema are in the document.
        for path in [
            "/v1/assets",
            "/v1/pools",
            "/v1/pools/detail",
            "/v1/pools/depth",
            "/v1/pairs/history",
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

    #[tokio::test]
    async fn swap_quote_rejects_zero_amount_and_same_token() {
        let token = Address::from([1; 20]);
        for body in [
            serde_json::json!({
                "token_in": token.to_string(),
                "token_out": Address::from([2; 20]).to_string(),
                "amount_in": "0",
            }),
            serde_json::json!({
                "token_in": token.to_string(),
                "token_out": token.to_string(),
                "amount_in": "1",
            }),
        ] {
            let (status, _) = post("/v1/swap/quote", body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn swap_bad_hex_is_400() {
        let (status, _) = post(
            "/v1/swap",
            serde_json::json!({
                "encodedOrder": "not-hex",
                "signature": "0x00",
                "chainId": 31337,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn swap_rejects_an_order_for_another_chain_before_decoding() {
        let (status, json) = post(
            "/v1/swap",
            serde_json::json!({
                "encodedOrder": "not-hex",
                "signature": "also-not-hex",
                "chainId": 1,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            json["error"],
            "order chainId does not match this deployment"
        );
    }

    #[tokio::test]
    async fn swap_undecodable_order_is_400() {
        // Well-formed hex that is not a V2 Dutch order: the cosigner fails to decode it.
        let (status, _) = post(
            "/v1/swap",
            serde_json::json!({
                "encodedOrder": "0x1234",
                "signature": "0x00",
                "chainId": 31337,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn trades_list_is_ok() {
        let (status, json) = get("/v1/trades").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "Ok");
    }

    #[tokio::test]
    async fn trades_bad_cursor_is_400() {
        let (status, _) = get("/v1/trades?cursor=!!!not-base64!!!").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn trades_bad_status_filter_is_400() {
        let (status, _) = get("/v1/trades?status=nonsense").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn trade_detail_unknown_is_404() {
        // A well-formed ULID with no trade behind it.
        let (status, _) = get("/v1/trades/00000000000000000000000000").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn trade_detail_malformed_id_is_400() {
        let (status, _) = get("/v1/trades/not-a-ulid").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn activity_feed_is_ok() {
        let (status, json) = get("/v1/activity").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "Ok");
    }

    #[tokio::test]
    async fn activity_bad_actor_is_400() {
        let (status, _) = get("/v1/activity?actor=not-an-address").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn stats_serves_tiles() {
        let (status, json) = get("/v1/stats").await;
        assert_eq!(status, StatusCode::OK);
        // Trade + event tiles are populated (zero over an empty store), not null.
        assert_eq!(json["result"]["trades_settled"], 0);
        assert_eq!(json["result"]["events_24h"], 0);
    }
}
