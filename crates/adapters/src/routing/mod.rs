//! Routing market data — a local cache of token prices (Binance `bookTicker` WebSocket) and
//! the chain gas price (RPC poll). The [`PriceOracle`] / [`GasPrice`] reads are a lock-guarded
//! map / scalar lookup with no network, so they stay off the quote hot path; two background
//! tasks keep the cache fresh and self-heal across dropped connections. USDT ≈ USD — a fast
//! estimate for the gas threshold, never a settlement price.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use alloy::{primitives::Address, providers::Provider};
use async_trait::async_trait;
use futures::StreamExt;
use parking_lot::RwLock;
use rust_decimal::Decimal;
use serde::Deserialize;
use solvent_core::deps::routing::{GasPrice, GasPriceError, PriceOracle, PriceOracleError};
use solvent_core::primitives::UsdPrice;
use tokio_tungstenite::tungstenite::Message;

/// Reconnect-backoff ceiling and stall timeout for the Binance socket.
const RECONNECT_MAX: Duration = Duration::from_secs(60);
const RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// The shared cache both ports read and both background feeders write.
#[derive(Default)]
pub struct MarketCache {
    prices: RwLock<HashMap<Address, UsdPrice>>,
    gas_wei: RwLock<u128>,
}

impl MarketCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    fn set_price(&self, token: Address, price: UsdPrice) {
        self.prices.write().insert(token, price);
    }
    fn set_gas(&self, wei: u128) {
        *self.gas_wei.write() = wei;
    }
}

#[async_trait]
impl PriceOracle for MarketCache {
    async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError> {
        self.prices
            .read()
            .get(&token)
            .copied()
            .ok_or(PriceOracleError::NotFound(token))
    }
}

#[async_trait]
impl GasPrice for MarketCache {
    async fn gas_price_wei(&self) -> Result<u128, GasPriceError> {
        match *self.gas_wei.read() {
            0 => Err(GasPriceError::Source(
                "gas price not yet fetched".to_string(),
            )),
            wei => Ok(wei),
        }
    }
}

/// Polls `eth_gasPrice` into the cache every `interval`. Spawn with `tokio::spawn`.
pub struct GasPoller<P> {
    provider: P,
    cache: Arc<MarketCache>,
    interval: Duration,
}

impl<P: Provider> GasPoller<P> {
    pub fn new(provider: P, cache: Arc<MarketCache>, interval: Duration) -> Self {
        Self {
            provider,
            cache,
            interval,
        }
    }
    pub async fn run(self) {
        loop {
            match self.provider.get_gas_price().await {
                Ok(wei) => self.cache.set_gas(wei),
                Err(err) => tracing::warn!(error = %err, "gas-price poll failed"),
            }
            tokio::time::sleep(self.interval).await;
        }
    }
}

/// Binance `bookTicker` WebSocket feed into the cache. One combined stream carries every
/// tracked symbol; each update writes a fresh mid `(bid + ask) / 2`. `symbols` maps a Binance
/// symbol (e.g. `ETHUSDT`) to every token it prices (WETH across chains all take `ETHUSDT`).
pub struct BinanceFeed {
    cache: Arc<MarketCache>,
    ws_base: String,
    symbols: HashMap<String, Vec<Address>>,
}

impl BinanceFeed {
    pub fn new(
        cache: Arc<MarketCache>,
        ws_base: String,
        symbols: HashMap<String, Vec<Address>>,
    ) -> Self {
        Self {
            cache,
            ws_base,
            symbols,
        }
    }

    /// Stream forever, reconnecting with exponential backoff. Returns if nothing is tracked.
    pub async fn run(self) {
        if self.symbols.is_empty() {
            return;
        }
        let mut backoff = Duration::from_secs(1);
        loop {
            if let Err(err) = self.stream().await {
                tracing::warn!(error = %err, "binance feed error; reconnecting");
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(RECONNECT_MAX);
        }
    }

    async fn stream(&self) -> Result<(), String> {
        let streams = self
            .symbols
            .keys()
            .map(|s| format!("{}@bookTicker", s.to_lowercase()))
            .collect::<Vec<_>>()
            .join("/");
        let url = format!("{}/stream?streams={streams}", self.ws_base);
        let (socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|e| e.to_string())?;
        let (_, mut read) = socket.split();
        while let Some(message) = tokio::time::timeout(RECV_TIMEOUT, read.next())
            .await
            .map_err(|e| e.to_string())?
        {
            if let Ok(Message::Text(text)) = message {
                self.ingest(&text);
            }
        }
        Ok(())
    }

    fn ingest(&self, text: &str) {
        let Ok(envelope) = serde_json::from_str::<Envelope>(text) else {
            return;
        };
        let Some(tokens) = self.symbols.get(&envelope.data.symbol) else {
            return;
        };
        if let Some(mid) = mid_price(&envelope.data) {
            for &token in tokens {
                self.cache.set_price(token, UsdPrice(mid));
            }
        }
    }
}

/// Combined-stream envelope: `{ "stream": …, "data": { … } }`.
#[derive(Deserialize)]
struct Envelope {
    data: BookTicker,
}

#[derive(Deserialize)]
struct BookTicker {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "b")]
    bid: String,
    #[serde(rename = "a")]
    ask: String,
}

/// `(bid + ask) / 2`, or `None` if either side doesn't parse.
fn mid_price(ticker: &BookTicker) -> Option<Decimal> {
    let bid: Decimal = ticker.bid.parse().ok()?;
    let ask: Decimal = ticker.ask.parse().ok()?;
    Some((bid + ask) / Decimal::from(2u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingests_book_ticker_as_mid_price() {
        let weth = Address::from([1u8; 20]);
        let cache = MarketCache::new();
        let feed = BinanceFeed::new(
            cache.clone(),
            "wss://x".to_string(),
            HashMap::from([("ETHUSDT".to_string(), vec![weth])]),
        );
        feed.ingest(
            r#"{"stream":"ethusdt@bookTicker","data":{"s":"ETHUSDT","b":"1999.5","a":"2000.5"}}"#,
        );
        assert_eq!(
            cache.prices.read().get(&weth).copied(),
            Some(UsdPrice(Decimal::from(2000u32)))
        );
    }
}
