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
    changes: RwLock<HashMap<Address, f64>>,
    gas_wei: RwLock<u128>,
}

impl MarketCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    fn set_price(&self, token: Address, price: UsdPrice) {
        self.prices.write().insert(token, price);
    }
    /// Seed a fixed price at boot — for a stablecoin pegged to USD that has no Binance pair (USDT).
    /// The feed never writes such a symbol, so the peg persists.
    pub fn seed_price(&self, token: Address, price: UsdPrice) {
        self.set_price(token, price);
    }
    fn set_change(&self, token: Address, pct: f64) {
        self.changes.write().insert(token, pct);
    }
    /// The token's 24h price-change percent (`2.5` = +2.5%), or `None` if unfed.
    pub fn change_24h(&self, token: Address) -> Option<f64> {
        self.changes.read().get(&token).copied()
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

/// Binance WebSocket feed into the cache. One combined stream carries every tracked symbol on two
/// channels: `@bookTicker` writes a fresh mid `(bid + ask) / 2`, `@ticker` writes the 24h price
/// change. `symbols` maps a Binance symbol (e.g. `ETHUSDT`) to every token it prices (WETH across
/// chains all take `ETHUSDT`).
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
            .flat_map(|s| {
                let s = s.to_lowercase();
                [format!("{s}@bookTicker"), format!("{s}@ticker")]
            })
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
        let Ok(env) = serde_json::from_str::<Envelope>(text) else {
            return;
        };
        if env.stream.ends_with("@bookTicker") {
            if let Ok(book) = serde_json::from_value::<BookTicker>(env.data) {
                self.apply_book(&book);
            }
        } else if env.stream.ends_with("@ticker") {
            if let Ok(stats) = serde_json::from_value::<Ticker24h>(env.data) {
                self.apply_change(&stats);
            }
        }
    }

    fn apply_book(&self, book: &BookTicker) {
        let Some(tokens) = self.symbols.get(&book.symbol) else {
            return;
        };
        if let Some(mid) = mid_price(book) {
            for &token in tokens {
                self.cache.set_price(token, UsdPrice(mid));
            }
        }
    }

    fn apply_change(&self, stats: &Ticker24h) {
        let Some(tokens) = self.symbols.get(&stats.symbol) else {
            return;
        };
        if let Ok(pct) = stats.change_pct.parse::<f64>() {
            for &token in tokens {
                self.cache.set_change(token, pct);
            }
        }
    }
}

/// Combined-stream envelope: `{ "stream": "ethusdt@ticker", "data": { … } }`. The `stream` suffix
/// selects how `data` is read.
#[derive(Deserialize)]
struct Envelope {
    stream: String,
    data: serde_json::Value,
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

/// The 24h rolling-window stats; `P` is the price-change percent.
#[derive(Deserialize)]
struct Ticker24h {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "P")]
    change_pct: String,
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
    fn seed_price_reads_back() {
        let usdt = Address::from([9u8; 20]);
        let cache = MarketCache::new();
        cache.seed_price(usdt, UsdPrice::PAR);
        assert_eq!(cache.prices.read().get(&usdt).copied(), Some(UsdPrice::PAR));
    }

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

    #[test]
    fn ingests_ticker_as_change() {
        let weth = Address::from([1u8; 20]);
        let cache = MarketCache::new();
        let feed = BinanceFeed::new(
            cache.clone(),
            "wss://x".to_string(),
            HashMap::from([("ETHUSDT".to_string(), vec![weth])]),
        );
        feed.ingest(r#"{"stream":"ethusdt@ticker","data":{"s":"ETHUSDT","P":"2.5"}}"#);
        assert_eq!(cache.change_24h(weth), Some(2.5));
    }
}
