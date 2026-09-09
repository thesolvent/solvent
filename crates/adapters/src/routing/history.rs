use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::primitives::Address;
use async_trait::async_trait;
use moka::future::Cache;
use serde_json::Value;
use solvent_core::asset::{PairPriceHistory, PairPricePoint, PriceHistoryPeriod};
use solvent_core::deps::asset::{PairPriceHistorySource, PairPriceHistorySourceError};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const HISTORY_LOAD_TIMEOUT: Duration = Duration::from_secs(20);
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const CACHE_CAPACITY: u64 = 256;
const KLINE_PAGE_SIZE: usize = 1_000;
const MAX_ALL_PAGES: usize = 16;
const HOUR_MS: u64 = 60 * 60 * 1_000;
const DAY_MS: u64 = 24 * HOUR_MS;

#[derive(Clone, Debug, PartialEq, Eq)]
enum MarketSource {
    Binance(String),
    FixedUsd,
}

#[derive(Clone, Debug)]
struct Kline {
    timestamp_ms: u64,
    close_usd: f64,
    volume_usd: f64,
}

#[derive(Clone, Debug)]
enum TokenHistory {
    FixedUsd,
    Series(Vec<Kline>),
}

#[derive(Clone, Copy)]
struct PeriodRequest {
    interval: &'static str,
    interval_ms: u64,
    start_time_ms: u64,
    paginate: bool,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct HistoryKey {
    base: Address,
    quote: Address,
    period: PriceHistoryPeriod,
}

struct BinanceClient {
    http: reqwest::Client,
    rest_url: String,
}

/// Cached Binance kline history for charting. It never participates in live valuation or order
/// routing, so an external history outage cannot delay quotes.
pub struct BinanceHistory {
    binance: BinanceClient,
    sources: HashMap<Address, MarketSource>,
    cache: Cache<HistoryKey, Arc<PairPriceHistory>>,
}

impl BinanceHistory {
    pub fn new(
        rest_url: String,
        symbols: HashMap<String, Vec<Address>>,
        fixed_usd_tokens: &[Address],
    ) -> Result<Self, PairPriceHistorySourceError> {
        let mut sources = HashMap::new();
        for (symbol, tokens) in symbols {
            for token in tokens {
                sources.insert(token, MarketSource::Binance(symbol.clone()));
            }
        }
        for token in fixed_usd_tokens {
            sources.insert(*token, MarketSource::FixedUsd);
        }
        Ok(Self {
            binance: BinanceClient::new(rest_url)?,
            sources,
            cache: Cache::builder()
                .max_capacity(CACHE_CAPACITY)
                .time_to_live(CACHE_TTL)
                .build(),
        })
    }

    async fn load(
        &self,
        base: Address,
        quote: Address,
        period: PriceHistoryPeriod,
    ) -> Result<PairPriceHistory, PairPriceHistorySourceError> {
        let now_ms = now_ms()?;
        let base_source = self.source(base)?.clone();
        let quote_source = self.source(quote)?.clone();
        let (base_history, quote_history) = if base_source == quote_source {
            let history = self.token_history(&base_source, period, now_ms).await?;
            (history.clone(), history)
        } else {
            tokio::try_join!(
                self.token_history(&base_source, period, now_ms),
                self.token_history(&quote_source, period, now_ms)
            )?
        };
        let points = pair_points(base_history, quote_history, now_ms)?;
        if points.is_empty() {
            return Err(PairPriceHistorySourceError::InvalidData(
                "the two price series have no common timestamps".to_string(),
            ));
        }
        Ok(PairPriceHistory {
            base,
            quote,
            period,
            points,
        })
    }

    fn source(&self, token: Address) -> Result<&MarketSource, PairPriceHistorySourceError> {
        self.sources
            .get(&token)
            .ok_or(PairPriceHistorySourceError::NotFound(token))
    }

    async fn token_history(
        &self,
        source: &MarketSource,
        period: PriceHistoryPeriod,
        now_ms: u64,
    ) -> Result<TokenHistory, PairPriceHistorySourceError> {
        match source {
            MarketSource::FixedUsd => Ok(TokenHistory::FixedUsd),
            MarketSource::Binance(symbol) => self
                .binance
                .fetch_symbol(symbol, period, now_ms)
                .await
                .map(TokenHistory::Series),
        }
    }
}

impl BinanceClient {
    fn new(rest_url: String) -> Result<Self, PairPriceHistorySourceError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(source_error)?;
        Ok(Self {
            http,
            rest_url: rest_url.trim_end_matches('/').to_string(),
        })
    }

    async fn fetch_symbol(
        &self,
        symbol: &str,
        period: PriceHistoryPeriod,
        now_ms: u64,
    ) -> Result<Vec<Kline>, PairPriceHistorySourceError> {
        let request = period_request(period, now_ms);
        let mut cursor = request.start_time_ms;
        let mut points = Vec::new();

        for page_index in 0..MAX_ALL_PAGES {
            let page = self
                .fetch_page(symbol, request.interval, cursor, KLINE_PAGE_SIZE)
                .await?;
            if page.is_empty() {
                break;
            }
            let page_len = page.len();
            let last_timestamp = page
                .last()
                .map(|point| point.timestamp_ms)
                .ok_or_else(|| invalid_data("non-empty kline page has no last element"))?;
            if last_timestamp < cursor {
                return Err(invalid_data("kline timestamps moved backwards"));
            }
            points.extend(
                page.into_iter()
                    .filter(|point| point.timestamp_ms <= now_ms),
            );

            if !request.paginate || page_len < KLINE_PAGE_SIZE || last_timestamp >= now_ms {
                break;
            }
            let next = last_timestamp
                .checked_add(request.interval_ms)
                .ok_or_else(|| invalid_data("kline timestamp overflow"))?;
            if next <= cursor {
                return Err(invalid_data("kline pagination did not advance"));
            }
            cursor = next;
            if page_index + 1 == MAX_ALL_PAGES {
                return Err(invalid_data("kline history exceeded the pagination limit"));
            }
        }

        validate_series(&points)?;
        Ok(points)
    }

    async fn fetch_page(
        &self,
        symbol: &str,
        interval: &str,
        start_time_ms: u64,
        limit: usize,
    ) -> Result<Vec<Kline>, PairPriceHistorySourceError> {
        let response = self
            .http
            .get(format!("{}/api/v3/klines", self.rest_url))
            .query(&[
                ("symbol", symbol.to_string()),
                ("interval", interval.to_string()),
                ("startTime", start_time_ms.to_string()),
                ("limit", limit.to_string()),
            ])
            .send()
            .await
            .map_err(source_error)?
            .error_for_status()
            .map_err(source_error)?;
        let rows: Vec<Vec<Value>> = response.json().await.map_err(source_error)?;
        rows.iter().map(|row| parse_kline(row)).collect()
    }
}

#[async_trait]
impl PairPriceHistorySource for BinanceHistory {
    async fn history(
        &self,
        base: Address,
        quote: Address,
        period: PriceHistoryPeriod,
    ) -> Result<PairPriceHistory, PairPriceHistorySourceError> {
        let key = HistoryKey {
            base,
            quote,
            period,
        };
        self.cache
            .try_get_with(key, async {
                tokio::time::timeout(HISTORY_LOAD_TIMEOUT, self.load(base, quote, period))
                    .await
                    .map_err(|_| {
                        PairPriceHistorySourceError::Source(
                            "historical price request timed out".to_string(),
                        )
                    })?
                    .map(Arc::new)
            })
            .await
            .map(|history| (*history).clone())
            .map_err(|error| (*error).clone())
    }
}

fn period_request(period: PriceHistoryPeriod, now_ms: u64) -> PeriodRequest {
    match period {
        PriceHistoryPeriod::SevenDays => PeriodRequest {
            interval: "1h",
            interval_ms: HOUR_MS,
            start_time_ms: now_ms.saturating_sub(7 * DAY_MS),
            paginate: false,
        },
        PriceHistoryPeriod::ThreeMonths => PeriodRequest {
            interval: "4h",
            interval_ms: 4 * HOUR_MS,
            start_time_ms: now_ms.saturating_sub(90 * DAY_MS),
            paginate: false,
        },
        PriceHistoryPeriod::All => PeriodRequest {
            interval: "1d",
            interval_ms: DAY_MS,
            start_time_ms: 0,
            paginate: true,
        },
    }
}

fn parse_kline(row: &[Value]) -> Result<Kline, PairPriceHistorySourceError> {
    let timestamp_ms = row
        .first()
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_data("kline open time is missing or invalid"))?;
    let close_usd = parse_number(row.get(4), "close price")?;
    let volume_usd = parse_number(row.get(7), "quote volume")?;
    if !close_usd.is_finite() || close_usd <= 0.0 {
        return Err(invalid_data(
            "kline close price must be finite and positive",
        ));
    }
    if !volume_usd.is_finite() || volume_usd < 0.0 {
        return Err(invalid_data(
            "kline quote volume must be finite and non-negative",
        ));
    }
    Ok(Kline {
        timestamp_ms,
        close_usd,
        volume_usd,
    })
}

fn parse_number(value: Option<&Value>, field: &str) -> Result<f64, PairPriceHistorySourceError> {
    value
        .and_then(Value::as_str)
        .and_then(|raw| raw.parse().ok())
        .ok_or_else(|| invalid_data(format!("kline {field} is missing or invalid")))
}

fn validate_series(points: &[Kline]) -> Result<(), PairPriceHistorySourceError> {
    if points.is_empty() {
        return Err(invalid_data("price source returned no klines"));
    }
    if points
        .windows(2)
        .any(|pair| pair[0].timestamp_ms >= pair[1].timestamp_ms)
    {
        return Err(invalid_data("kline timestamps must be strictly increasing"));
    }
    Ok(())
}

fn pair_points(
    base: TokenHistory,
    quote: TokenHistory,
    now_ms: u64,
) -> Result<Vec<PairPricePoint>, PairPriceHistorySourceError> {
    match (base, quote) {
        (TokenHistory::FixedUsd, TokenHistory::FixedUsd) => {
            Ok(vec![pair_point(now_ms, 1.0, None)?])
        }
        (TokenHistory::Series(base), TokenHistory::FixedUsd) => base
            .into_iter()
            .map(|point| pair_point(point.timestamp_ms, point.close_usd, Some(point.volume_usd)))
            .collect(),
        (TokenHistory::FixedUsd, TokenHistory::Series(quote)) => quote
            .into_iter()
            .map(|point| {
                pair_point(
                    point.timestamp_ms,
                    1.0 / point.close_usd,
                    Some(point.volume_usd),
                )
            })
            .collect(),
        (TokenHistory::Series(base), TokenHistory::Series(quote)) => align_series(&base, &quote),
    }
}

fn align_series(
    base: &[Kline],
    quote: &[Kline],
) -> Result<Vec<PairPricePoint>, PairPriceHistorySourceError> {
    let mut points = Vec::with_capacity(base.len().min(quote.len()));
    let (mut left, mut right) = (0, 0);
    while left < base.len() && right < quote.len() {
        match base[left].timestamp_ms.cmp(&quote[right].timestamp_ms) {
            std::cmp::Ordering::Less => left += 1,
            std::cmp::Ordering::Greater => right += 1,
            std::cmp::Ordering::Equal => {
                points.push(pair_point(
                    base[left].timestamp_ms,
                    base[left].close_usd / quote[right].close_usd,
                    Some(base[left].volume_usd),
                )?);
                left += 1;
                right += 1;
            }
        }
    }
    Ok(points)
}

fn pair_point(
    timestamp_ms: u64,
    price: f64,
    volume_usd: Option<f64>,
) -> Result<PairPricePoint, PairPriceHistorySourceError> {
    if !price.is_finite() || price <= 0.0 {
        return Err(invalid_data("pair price must be finite and positive"));
    }
    Ok(PairPricePoint {
        timestamp_ms,
        price,
        volume_usd,
    })
}

fn now_ms() -> Result<u64, PairPriceHistorySourceError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(source_error)?
        .as_millis();
    u64::try_from(millis).map_err(|_| invalid_data("system time exceeds uint64 milliseconds"))
}

fn source_error(error: impl std::fmt::Display) -> PairPriceHistorySourceError {
    PairPriceHistorySourceError::Source(error.to_string())
}

fn invalid_data(reason: impl Into<String>) -> PairPriceHistorySourceError {
    PairPriceHistorySourceError::InvalidData(reason.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kline(timestamp_ms: u64, close_usd: f64, volume_usd: f64) -> Kline {
        Kline {
            timestamp_ms,
            close_usd,
            volume_usd,
        }
    }

    #[test]
    fn parses_the_documented_binance_kline_shape() {
        let row = vec![
            Value::from(1_499_040_000_000u64),
            Value::from("0.01634790"),
            Value::from("0.80000000"),
            Value::from("0.01575800"),
            Value::from("0.01577100"),
            Value::from("148.97600000"),
            Value::from(1_499_054_399_999u64),
            Value::from("2434.19055334"),
        ];

        let parsed = parse_kline(&row).expect("documented kline is valid");

        assert_eq!(parsed.timestamp_ms, 1_499_040_000_000);
        assert_eq!(parsed.close_usd, 0.015771);
        assert_eq!(parsed.volume_usd, 2434.19055334);
    }

    #[test]
    fn rejects_zero_non_finite_and_out_of_order_market_data() {
        let zero = vec![
            Value::from(1u64),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::from("0"),
            Value::Null,
            Value::Null,
            Value::from("1"),
        ];
        assert!(parse_kline(&zero).is_err());
        assert!(validate_series(&[kline(2, 1.0, 1.0), kline(1, 1.0, 1.0)]).is_err());
        assert!(pair_point(1, f64::INFINITY, None).is_err());
    }

    #[test]
    fn computes_pair_prices_and_keeps_only_aligned_timestamps() {
        let base = TokenHistory::Series(vec![
            kline(1, 80_000.0, 10.0),
            kline(2, 81_000.0, 20.0),
            kline(4, 82_000.0, 30.0),
        ]);
        let quote = TokenHistory::Series(vec![
            kline(1, 1.0, 100.0),
            kline(3, 1.0, 200.0),
            kline(4, 1.0001, 300.0),
        ]);

        let points = pair_points(base, quote, 10).expect("valid series combine");

        assert_eq!(points.len(), 2);
        assert_eq!(points[0].timestamp_ms, 1);
        assert_eq!(points[0].price, 80_000.0);
        assert!((points[1].price - 81_991.80081991802).abs() < 1e-9);
        assert_eq!(points[1].volume_usd, Some(30.0));
    }

    #[test]
    fn inverts_a_fixed_base_against_a_fetched_quote() {
        let points = pair_points(
            TokenHistory::FixedUsd,
            TokenHistory::Series(vec![kline(1, 80_000.0, 25.0)]),
            10,
        )
        .expect("fixed/fetched pair combines");

        assert!((points[0].price - 0.0000125).abs() < f64::EPSILON);
        assert_eq!(points[0].volume_usd, Some(25.0));
    }

    #[test]
    fn period_requests_use_real_windows_and_paginate_only_all_history() {
        let now = 100 * DAY_MS;
        let week = period_request(PriceHistoryPeriod::SevenDays, now);
        let quarter = period_request(PriceHistoryPeriod::ThreeMonths, now);
        let all = period_request(PriceHistoryPeriod::All, now);

        assert_eq!(week.start_time_ms, 93 * DAY_MS);
        assert_eq!(week.interval, "1h");
        assert!(!week.paginate);
        assert_eq!(quarter.start_time_ms, 10 * DAY_MS);
        assert_eq!(quarter.interval, "4h");
        assert!(!quarter.paginate);
        assert_eq!(all.start_time_ms, 0);
        assert_eq!(all.interval, "1d");
        assert!(all.paginate);
    }

    #[tokio::test]
    async fn fixed_pairs_are_served_without_network_and_cached_by_period() {
        let base = Address::from([1; 20]);
        let quote = Address::from([2; 20]);
        let history = BinanceHistory::new(
            "http://unused.invalid".to_string(),
            HashMap::new(),
            &[base, quote],
        )
        .expect("client configuration is valid");

        let first = history
            .history(base, quote, PriceHistoryPeriod::SevenDays)
            .await
            .expect("fixed pair is available");
        let second = history
            .history(base, quote, PriceHistoryPeriod::SevenDays)
            .await
            .expect("cached fixed pair is available");
        history.cache.run_pending_tasks().await;

        assert_eq!(first, second);
        assert_eq!(first.points.len(), 1);
        assert_eq!(first.points[0].price, 1.0);
        assert_eq!(history.cache.entry_count(), 1);
    }

    #[tokio::test]
    async fn rejects_tokens_without_a_configured_market_source() {
        let history = BinanceHistory::new("http://unused.invalid".to_string(), HashMap::new(), &[])
            .expect("client configuration is valid");

        let error = history
            .history(
                Address::from([1; 20]),
                Address::from([2; 20]),
                PriceHistoryPeriod::SevenDays,
            )
            .await
            .expect_err("unknown tokens must fail");

        assert!(matches!(error, PairPriceHistorySourceError::NotFound(_)));
    }
}
