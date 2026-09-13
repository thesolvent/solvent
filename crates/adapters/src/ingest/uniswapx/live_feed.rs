//! The read-only UniswapX feed simulation and its SQLite record.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, U256};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use solvent_core::deps::ingest::Normalizer;
use solvent_core::deps::routing::PriceOracle;
use solvent_core::obs::{debug, warn};
use solvent_core::primitives::ingest::Intent;
use solvent_core::primitives::{IntentId, UsdPrice};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqlitePool};
use thiserror::Error;
use tokio::time::{self, Instant, MissedTickBehavior};

use super::orders_api::{OrderRecord, OrdersApiClient};
use super::{
    SimulatedBatchPool, SimulatedQuoteEngine, SimulatedQuoteRequest, UniswapXV2Normalizer,
};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const Q18: u64 = 1_000_000_000_000_000_000;

/// A public order token identity and its display metadata.
#[derive(Clone, Debug)]
pub struct UniswapFeedAsset {
    pub source_address: Address,
    pub symbol: String,
    pub logo_uri: Option<String>,
    pub decimals: u8,
}

/// A stable continuation point in the descending feed order.
#[derive(Clone, Copy)]
pub struct UniswapXFeedCursor {
    pub last_seen_at: u64,
    pub order_hash: IntentId,
}

/// One persisted UniswapX order simulation.
#[derive(Clone, Debug)]
pub struct UniswapXFeedOrder {
    pub order_hash: IntentId,
    pub source_chain_id: u64,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub required_out: U256,
    pub market_out_per_in_q18: U256,
    pub simulated_amount_out: U256,
    pub simulated_batch_id: u64,
    pub observed_at: u64,
    pub last_seen_at: u64,
}

/// SQLite storage for the first simulation of each public UniswapX order.
#[derive(Clone)]
pub struct SqliteUniswapXFeedStore {
    pool: SqlitePool,
}

impl SqliteUniswapXFeedStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Apply the shared SQLite migration set.
    #[cfg(test)]
    async fn migrate(&self) -> Result<(), FeedStoreError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(FeedStoreError::database)
    }

    async fn contains(&self, order_hash: IntentId) -> Result<bool, FeedStoreError> {
        sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM uniswapx_feed_order WHERE order_hash = ?)",
        )
        .bind(order_hash.0.as_slice())
        .fetch_one(&self.pool)
        .await
        .map(|exists| exists != 0)
        .map_err(FeedStoreError::database)
    }

    async fn touch(&self, order_hash: IntentId, seen_at: u64) -> Result<(), FeedStoreError> {
        sqlx::query("UPDATE uniswapx_feed_order SET last_seen_at = ? WHERE order_hash = ?")
            .bind(i64_of(seen_at)?)
            .bind(order_hash.0.as_slice())
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(FeedStoreError::database)
    }

    async fn insert(&self, order: &SimulatedFeedOrder) -> Result<(), FeedStoreError> {
        sqlx::query(
            "INSERT INTO uniswapx_feed_order (
                 order_hash, source_chain_id, token_in, token_out, amount_in, required_out,
                 market_out_per_in_q18, simulated_amount_out, simulated_batch_id, observed_at,
                 last_seen_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(order.order_hash.0.as_slice())
        .bind(i64::try_from(order.source_chain_id).map_err(|_| FeedStoreError::Value("chain id"))?)
        .bind(order.token_in.as_slice())
        .bind(order.token_out.as_slice())
        .bind(order.amount_in.to_string())
        .bind(order.required_out.to_string())
        .bind(order.market_out_per_in_q18.to_string())
        .bind(order.simulated_amount_out.to_string())
        .bind(i64_of(order.simulated_batch_id)?)
        .bind(i64_of(order.observed_at)?)
        .bind(i64_of(order.observed_at)?)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(FeedStoreError::database)
    }

    /// Lists simulated orders newest-first, with the hash breaking timestamp ties.
    pub async fn list(
        &self,
        cursor: Option<UniswapXFeedCursor>,
        limit: u32,
    ) -> Result<Vec<UniswapXFeedOrder>, FeedStoreError> {
        let rows = match cursor {
            Some(cursor) => {
                sqlx::query(
                    "SELECT * FROM uniswapx_feed_order
                     WHERE last_seen_at < ? OR (last_seen_at = ? AND order_hash < ?)
                     ORDER BY last_seen_at DESC, order_hash DESC
                     LIMIT ?",
                )
                .bind(i64_of(cursor.last_seen_at)?)
                .bind(i64_of(cursor.last_seen_at)?)
                .bind(cursor.order_hash.0.as_slice())
                .bind(i64::from(limit))
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query(
                    "SELECT * FROM uniswapx_feed_order
                     ORDER BY last_seen_at DESC, order_hash DESC
                     LIMIT ?",
                )
                .bind(i64::from(limit))
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(FeedStoreError::database)?;
        rows.iter().map(row_to_feed_order).collect()
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FeedStoreError {
    #[error("UniswapX feed database: {0}")]
    Database(String),
    #[error("UniswapX feed contains invalid data: {0}")]
    Data(String),
    #[error("UniswapX feed value exceeds SQLite integer range: {0}")]
    Value(&'static str),
}

impl FeedStoreError {
    fn database(error: impl std::fmt::Display) -> Self {
        Self::Database(error.to_string())
    }

    fn data(error: impl std::fmt::Display) -> Self {
        Self::Data(error.to_string())
    }
}

struct SimulatedFeedOrder {
    order_hash: IntentId,
    source_chain_id: u64,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    required_out: U256,
    market_out_per_in_q18: U256,
    simulated_amount_out: U256,
    simulated_batch_id: u64,
    observed_at: u64,
}

/// Polls the public book at its four-requests-per-second limit and records one local simulation for
/// each configured pair. It has no route to the normal order, trade, or fill services.
pub struct UniswapXFeedWorker {
    client: OrdersApiClient,
    assets: HashMap<Address, UniswapFeedAsset>,
    prices: Arc<dyn PriceOracle>,
    batches: SimulatedBatchPool,
    quotes: SimulatedQuoteEngine,
    store: SqliteUniswapXFeedStore,
}

impl UniswapXFeedWorker {
    pub fn new(
        client: OrdersApiClient,
        assets: impl IntoIterator<Item = UniswapFeedAsset>,
        prices: Arc<dyn PriceOracle>,
        batches: SimulatedBatchPool,
        store: SqliteUniswapXFeedStore,
    ) -> Self {
        Self {
            client,
            assets: assets
                .into_iter()
                .map(|asset| (asset.source_address, asset))
                .collect(),
            prices,
            batches,
            quotes: SimulatedQuoteEngine::new(),
            store,
        }
    }

    /// Runs one request at most every 250ms. Delayed ticks prevent a slow response from creating a
    /// compensating burst that would exceed the Orders API's four-request-per-second limit.
    pub async fn run(self) {
        let mut polls = time::interval_at(Instant::now() + POLL_INTERVAL, POLL_INTERVAL);
        polls.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            polls.tick().await;
            let records = match self.client.open_orders().await {
                Ok(records) => records,
                Err(error) => {
                    let _ = &error;
                    warn!("UniswapX feed poll failed: {error}");
                    continue;
                }
            };
            let observed_at = now_unix();
            for record in records {
                if let Err(error) = self.observe(record, observed_at).await {
                    let _ = &error;
                    warn!("UniswapX feed order could not be recorded: {error}");
                }
            }
        }
    }

    async fn observe(&self, record: OrderRecord, observed_at: u64) -> Result<(), FeedStoreError> {
        let raw = match record.raw(observed_at) {
            Ok(raw) => raw,
            Err(error) => {
                let _ = &error;
                debug!("UniswapX feed dropped malformed record: {error}");
                return Ok(());
            }
        };
        self.observe_raw(raw, observed_at).await
    }

    async fn observe_raw(
        &self,
        raw: solvent_core::primitives::ingest::RawOrder,
        observed_at: u64,
    ) -> Result<(), FeedStoreError> {
        let intent = match UniswapXV2Normalizer.normalize(&raw) {
            Ok(intent) => intent,
            Err(_) => {
                debug!("UniswapX feed dropped undecodable Dutch-V2 order");
                return Ok(());
            }
        };
        if self.store.contains(intent.id).await? {
            return self.store.touch(intent.id, observed_at).await;
        }
        let Some(order) = self.simulate(&intent, observed_at).await else {
            return Ok(());
        };
        self.store.insert(&order).await
    }

    async fn simulate(&self, intent: &Intent, observed_at: u64) -> Option<SimulatedFeedOrder> {
        let [output] = intent.outputs.as_slice() else {
            return None;
        };
        let input_asset = self.assets.get(&intent.input.token)?;
        let output_asset = self.assets.get(&output.token)?;
        let amount_in = intent.input.curve.amount_at(observed_at);
        let required_out = output.curve.amount_at(observed_at);
        if amount_in.is_zero() || required_out.is_zero() {
            return None;
        }
        let input_price = self.prices.price(input_asset.source_address).await.ok()?;
        let output_price = self.prices.price(output_asset.source_address).await.ok()?;
        let market_out_per_in_q18 = market_out_per_in_q18(input_price, output_price)?;
        let batch = self.batches.claim();
        let quote = self.quotes.quote(
            &batch,
            SimulatedQuoteRequest {
                token_in: input_asset.source_address,
                token_out: output_asset.source_address,
                amount_in,
                input_decimals: input_asset.decimals,
                output_decimals: output_asset.decimals,
                market_out_per_in_q18,
            },
        )?;
        Some(SimulatedFeedOrder {
            order_hash: intent.id,
            source_chain_id: intent.origin_chain.0,
            token_in: intent.input.token,
            token_out: output.token,
            amount_in,
            required_out,
            market_out_per_in_q18,
            simulated_amount_out: quote.amount_out,
            simulated_batch_id: quote.batch_id,
            observed_at,
        })
    }
}

fn market_out_per_in_q18(input: UsdPrice, output: UsdPrice) -> Option<U256> {
    if input.0 <= Decimal::ZERO || output.0 <= Decimal::ZERO {
        return None;
    }
    input
        .0
        .checked_div(output.0)?
        .checked_mul(Decimal::from(Q18))?
        .trunc()
        .to_u128()
        .map(U256::from)
        .filter(|price| !price.is_zero())
}

fn row_to_feed_order(row: &SqliteRow) -> Result<UniswapXFeedOrder, FeedStoreError> {
    Ok(UniswapXFeedOrder {
        order_hash: IntentId(hash(row, "order_hash")?),
        source_chain_id: count(row, "source_chain_id")?,
        token_in: address(row, "token_in")?,
        token_out: address(row, "token_out")?,
        amount_in: amount(row, "amount_in")?,
        required_out: amount(row, "required_out")?,
        market_out_per_in_q18: amount(row, "market_out_per_in_q18")?,
        simulated_amount_out: amount(row, "simulated_amount_out")?,
        simulated_batch_id: count(row, "simulated_batch_id")?,
        observed_at: count(row, "observed_at")?,
        last_seen_at: count(row, "last_seen_at")?,
    })
}

fn address(row: &SqliteRow, column: &str) -> Result<Address, FeedStoreError> {
    let bytes: Vec<u8> = row.try_get(column).map_err(FeedStoreError::database)?;
    <[u8; 20]>::try_from(bytes.as_slice())
        .map(Address::from)
        .map_err(|_| FeedStoreError::data(format!("column '{column}' must contain 20 bytes")))
}

fn hash(row: &SqliteRow, column: &str) -> Result<alloy::primitives::B256, FeedStoreError> {
    let bytes: Vec<u8> = row.try_get(column).map_err(FeedStoreError::database)?;
    alloy::primitives::B256::try_from(bytes.as_slice())
        .map_err(|_| FeedStoreError::data(format!("column '{column}' must contain 32 bytes")))
}

fn amount(row: &SqliteRow, column: &str) -> Result<U256, FeedStoreError> {
    let text: String = row.try_get(column).map_err(FeedStoreError::database)?;
    U256::from_str(&text)
        .map_err(|_| FeedStoreError::data(format!("column '{column}' must contain a uint256")))
}

fn count(row: &SqliteRow, column: &str) -> Result<u64, FeedStoreError> {
    let value: i64 = row.try_get(column).map_err(FeedStoreError::database)?;
    u64::try_from(value)
        .map_err(|_| FeedStoreError::data(format!("column '{column}' must not be negative")))
}

fn i64_of(value: u64) -> Result<i64, FeedStoreError> {
    i64::try_from(value).map_err(|_| FeedStoreError::Value("timestamp or batch id"))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use alloy::primitives::{address, B256};
    use alloy::signers::local::PrivateKeySigner;
    use async_trait::async_trait;
    use solvent_core::deps::routing::PriceOracleError;
    use solvent_core::primitives::ingest::RawOrder;
    use sqlx::Row;

    use super::super::{OrderSpec, SignedOrderBuilder};
    use super::*;

    const PERMIT2: Address = address!("000000000022D473030F116dDEE9F6B43aC78BA3");
    const SOURCE_IN: Address = address!("1111111111111111111111111111111111111111");
    const SOURCE_OUT: Address = address!("2222222222222222222222222222222222222222");

    struct Prices(HashMap<Address, UsdPrice>);

    #[async_trait]
    impl PriceOracle for Prices {
        async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError> {
            self.0
                .get(&token)
                .copied()
                .ok_or(PriceOracleError::NotFound(token))
        }
    }

    fn signer(byte: u8) -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::repeat_byte(byte)).expect("valid test key")
    }

    fn order() -> RawOrder {
        SignedOrderBuilder::new(PERMIT2, 1, signer(0x11), signer(0x22)).build(
            &OrderSpec {
                reactor: address!("5555555555555555555555555555555555555555"),
                nonce: U256::from(1u64),
                deadline: 2_000,
                input_token: SOURCE_IN,
                input_start: U256::from(1_000_000_000_000_000_000u128),
                input_end: U256::from(1_000_000_000_000_000_000u128),
                output_token: SOURCE_OUT,
                output_start: U256::from(900_000_000_000_000_000u128),
                output_end: U256::from(900_000_000_000_000_000u128),
                recipient: address!("6666666666666666666666666666666666666666"),
                decay_start: 1_000,
                decay_end: 1_500,
                exclusive_filler: Address::ZERO,
            },
            1_000,
        )
    }

    #[tokio::test]
    async fn feed_rows_page_by_last_seen_and_order_hash() {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory database");
        let store = SqliteUniswapXFeedStore::new(pool);
        store.migrate().await.expect("migrations apply");
        for (byte, last_seen_at) in [(1, 100), (3, 100), (2, 99)] {
            store
                .insert(&SimulatedFeedOrder {
                    order_hash: IntentId(B256::repeat_byte(byte)),
                    source_chain_id: 1,
                    token_in: SOURCE_IN,
                    token_out: SOURCE_OUT,
                    amount_in: U256::from(1u8),
                    required_out: U256::from(1u8),
                    market_out_per_in_q18: U256::from(Q18),
                    simulated_amount_out: U256::from(1u8),
                    simulated_batch_id: 0,
                    observed_at: last_seen_at,
                })
                .await
                .expect("feed order inserts");
        }

        let first = store.list(None, 2).await.expect("first page");
        assert_eq!(
            first
                .iter()
                .map(|order| order.order_hash.0)
                .collect::<Vec<_>>(),
            vec![B256::repeat_byte(3), B256::repeat_byte(1)]
        );
        let cursor = first.last().map(|order| UniswapXFeedCursor {
            last_seen_at: order.last_seen_at,
            order_hash: order.order_hash,
        });
        let second = store.list(cursor, 2).await.expect("second page");
        assert_eq!(
            second
                .iter()
                .map(|order| order.order_hash.0)
                .collect::<Vec<_>>(),
            vec![B256::repeat_byte(2)]
        );
    }

    #[tokio::test]
    async fn configured_order_is_simulated_once_and_duplicate_only_updates_last_seen() {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory database");
        let store = SqliteUniswapXFeedStore::new(pool.clone());
        store.migrate().await.expect("migrations apply");
        let prices: Arc<dyn PriceOracle> = Arc::new(Prices(HashMap::from([
            (SOURCE_IN, UsdPrice(Decimal::from(2u64))),
            (SOURCE_OUT, UsdPrice(Decimal::ONE)),
        ])));
        let worker = UniswapXFeedWorker::new(
            OrdersApiClient::mainnet().expect("orders client"),
            [
                UniswapFeedAsset {
                    source_address: SOURCE_IN,
                    symbol: "IN".to_string(),
                    logo_uri: Some("https://example.com/in.svg".to_string()),
                    decimals: 18,
                },
                UniswapFeedAsset {
                    source_address: SOURCE_OUT,
                    symbol: "OUT".to_string(),
                    logo_uri: Some("https://example.com/out.svg".to_string()),
                    decimals: 18,
                },
            ],
            prices,
            SimulatedBatchPool::seeded(7, 1),
            store,
        );
        let raw = order();

        worker
            .observe_raw(raw.clone(), 1_000)
            .await
            .expect("first observation records a quote");
        worker
            .observe_raw(raw, 1_005)
            .await
            .expect("duplicate only updates the observation time");

        let row = sqlx::query(
            "SELECT COUNT(*) AS count, simulated_batch_id, last_seen_at FROM uniswapx_feed_order",
        )
        .fetch_one(&pool)
        .await
        .expect("read feed record");
        assert_eq!(row.get::<i64, _>("count"), 1);
        assert!(row.get::<i64, _>("simulated_batch_id") >= 0);
        assert_eq!(row.get::<i64, _>("last_seen_at"), 1_005);
    }
}
