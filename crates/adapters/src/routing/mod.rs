//! Routing market data — a local cache of token prices (Binance `bookTicker` WebSocket) and
//! the chain gas price (RPC poll). The [`PriceOracle`] / [`GasPrice`] reads are a lock-guarded
//! map / scalar lookup with no network, so they stay off the quote hot path; two background
//! tasks keep the cache fresh and self-heal across dropped connections. USDT ≈ USD — a fast
//! estimate for the gas threshold, never a settlement price.

mod history;

pub use history::BinanceHistory;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::{
    primitives::{Address, U256},
    providers::Provider,
};
use async_trait::async_trait;
use futures::StreamExt;
use parking_lot::RwLock;
use rust_decimal::Decimal;
use serde::Deserialize;
use solvent_core::deps::rebate::{RebateMarketBook, RebateMarketBookError, RebateMarketRequest};
use solvent_core::deps::routing::{GasPrice, GasPriceError, PriceOracle, PriceOracleError};
use solvent_core::primitives::pricing::Ratio;
use solvent_core::primitives::rebate::RebateMarket;
use solvent_core::primitives::UsdPrice;
use tokio_tungstenite::tungstenite::Message;

/// Reconnect-backoff ceiling and stall timeout for the Binance socket.
const RECONNECT_MAX: Duration = Duration::from_secs(60);
const RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// The shared cache both ports read and both background feeders write.
#[derive(Default)]
pub struct MarketCache {
    prices: RwLock<HashMap<Address, UsdPrice>>,
    books: RwLock<HashMap<Address, BookPrice>>,
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
    /// Hold a token at par from boot — for a stablecoin with no Binance pair. Pegged by
    /// configuration means it does not move, so its daily change is zero rather than unknown; the
    /// feed never writes such a symbol, so both persist.
    pub fn seed_peg(&self, token: Address) {
        self.set_price(token, UsdPrice::PAR);
        self.books.write().insert(
            token,
            BookPrice {
                bid: Decimal::ONE,
                ask: Decimal::ONE,
                observed_at: None,
            },
        );
        self.set_change(token, 0.0);
    }
    fn set_change(&self, token: Address, pct: f64) {
        self.changes.write().insert(token, pct);
    }
    fn set_gas(&self, wei: u128) {
        *self.gas_wei.write() = wei;
    }

    fn set_book(&self, token: Address, bid: Decimal, ask: Decimal, observed_at: u64) {
        self.books.write().insert(
            token,
            BookPrice {
                bid,
                ask,
                observed_at: Some(observed_at),
            },
        );
    }
}

#[derive(Clone, Copy)]
struct BookPrice {
    bid: Decimal,
    ask: Decimal,
    /// `None` is a configured fixed peg and therefore does not age out.
    observed_at: Option<u64>,
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

    async fn change_24h(&self, token: Address) -> Option<f64> {
        self.changes.read().get(&token).copied()
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

#[async_trait]
impl RebateMarketBook for MarketCache {
    async fn market(
        &self,
        request: RebateMarketRequest,
    ) -> Result<RebateMarket, RebateMarketBookError> {
        if request.token_a == request.token_b {
            return Err(RebateMarketBookError::Invalid("tokens are identical"));
        }
        let now = now_unix();
        let books = self.books.read();
        let a = fresh_book(
            request.token_a,
            books.get(&request.token_a).copied(),
            now,
            request.max_age_secs,
        )?;
        let b = fresh_book(
            request.token_b,
            books.get(&request.token_b).copied(),
            now,
            request.max_age_secs,
        )?;
        let a_to_b = output_per_input(
            a.bid,
            request.token_a_decimals,
            b.ask,
            request.token_b_decimals,
        )?;
        let b_to_a = output_per_input(
            b.bid,
            request.token_b_decimals,
            a.ask,
            request.token_a_decimals,
        )?;
        Ok(RebateMarket::new(
            request.token_a,
            request.token_b,
            a_to_b,
            b_to_a,
        ))
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
        if let Some((bid, ask)) = book_price(book) {
            let mid = (bid + ask) / Decimal::from(2u8);
            let observed_at = now_unix();
            for &token in tokens {
                self.cache.set_price(token, UsdPrice(mid));
                self.cache.set_book(token, bid, ask, observed_at);
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

/// A crossed, zero, or malformed external book is unusable for both display and execution.
fn book_price(ticker: &BookTicker) -> Option<(Decimal, Decimal)> {
    let bid: Decimal = ticker.bid.parse().ok()?;
    let ask: Decimal = ticker.ask.parse().ok()?;
    (bid > Decimal::ZERO && ask >= bid).then_some((bid, ask))
}

fn fresh_book(
    token: Address,
    book: Option<BookPrice>,
    now: u64,
    max_age_secs: u64,
) -> Result<BookPrice, RebateMarketBookError> {
    let book = book.ok_or(RebateMarketBookError::NotFound(token))?;
    if book
        .observed_at
        .is_some_and(|observed| now.saturating_sub(observed) > max_age_secs)
    {
        return Err(RebateMarketBookError::Stale(token));
    }
    Ok(book)
}

fn output_per_input(
    input_bid: Decimal,
    input_decimals: u8,
    output_ask: Decimal,
    output_decimals: u8,
) -> Result<Ratio, RebateMarketBookError> {
    let input_mantissa = positive_mantissa(input_bid)?;
    let output_mantissa = positive_mantissa(output_ask)?;
    let numerator = input_mantissa
        .checked_mul(pow10(u32::from(output_decimals) + output_ask.scale())?)
        .ok_or(RebateMarketBookError::Invalid(
            "price ratio exceeds uint256",
        ))?;
    let denominator = output_mantissa
        .checked_mul(pow10(u32::from(input_decimals) + input_bid.scale())?)
        .ok_or(RebateMarketBookError::Invalid(
            "price ratio exceeds uint256",
        ))?;
    Ratio::new(numerator, denominator).ok_or(RebateMarketBookError::Invalid(
        "price ratio denominator is zero",
    ))
}

fn positive_mantissa(value: Decimal) -> Result<U256, RebateMarketBookError> {
    u128::try_from(value.mantissa())
        .map(U256::from)
        .map_err(|_| RebateMarketBookError::Invalid("book prices must be positive"))
}

fn pow10(exponent: u32) -> Result<U256, RebateMarketBookError> {
    (0..exponent).try_fold(U256::from(1u8), |value, _| {
        value
            .checked_mul(U256::from(10u8))
            .ok_or(RebateMarketBookError::Invalid(
                "token scale exceeds uint256",
            ))
    })
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_peg_holds_par_and_reports_no_move() {
        let usdt = Address::from([9u8; 20]);
        let cache = MarketCache::new();
        cache.seed_peg(usdt);
        assert_eq!(cache.prices.read().get(&usdt).copied(), Some(UsdPrice::PAR));
        // A configured peg has not moved, which is different from having no reading.
        assert_eq!(cache.changes.read().get(&usdt).copied(), Some(0.0));
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
        let book = cache.books.read()[&weth];
        assert_eq!(book.bid, Decimal::new(19_995, 1));
        assert_eq!(book.ask, Decimal::new(20_005, 1));
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
        assert_eq!(cache.changes.read().get(&weth).copied(), Some(2.5));
    }

    #[tokio::test]
    async fn rebate_market_uses_the_adverse_side_and_rejects_stale_books() {
        let token_a = Address::from([1u8; 20]);
        let token_b = Address::from([2u8; 20]);
        let cache = MarketCache::new();
        let now = now_unix();
        cache.set_book(token_a, Decimal::new(19, 1), Decimal::new(21, 1), now);
        cache.set_book(token_b, Decimal::new(99, 2), Decimal::new(101, 2), now);
        let request = RebateMarketRequest::new(token_a, 6, token_b, 6, 5);

        let market = cache.market(request).await.unwrap();
        assert_eq!(
            (Ratio::from(U256::from(1_000_000u64)) * market.a_to_b)
                .floor()
                .unwrap(),
            U256::from(1_881_188u64)
        );
        assert_eq!(
            (Ratio::from(U256::from(1_000_000u64)) * market.b_to_a)
                .floor()
                .unwrap(),
            U256::from(471_428u64)
        );

        cache.set_book(token_a, Decimal::ONE, Decimal::ONE, 1);
        assert!(matches!(
            cache.market(request).await,
            Err(RebateMarketBookError::Stale(token)) if token == token_a
        ));
    }
}
