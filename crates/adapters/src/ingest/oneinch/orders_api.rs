//! A read-only client for 1inch's Orderbook API (`api.1inch.dev/orderbook/v4.1`) — the only place
//! an unfilled 1inch Limit Order Protocol order exists off-chain, exactly as the Orders API is for
//! UniswapX.
//!
//! Two constraints, each found by hitting the live endpoint rather than assumed from docs:
//!
//! - **Bearer API key required.** Unlike UniswapX's feed, there is no anonymous tier; a request
//!   with no or an invalid key comes back `401`.
//! - **The default `reqwest` user agent is refused by the edge's WAF with a `403`**, the opposite
//!   problem UniswapX's client has (which *requires* an identifiable one) — a generic-looking
//!   agent string passes.
//!
//! Pagination is cursor-based (`meta.hasMore`/`meta.nextCursor`), not page-numbered.

use std::time::Duration;

use alloy::primitives::{Address, Bytes, U256};
use serde::Deserialize;
use thiserror::Error;

use solvent_core::primitives::ingest::{OrderSource, ProtocolId, RawOrder};
use solvent_core::primitives::ChainId;

use super::codec::WireOrder;
use alloy::sol_types::SolValue;

/// A generic-looking agent string the edge's WAF does not refuse. Confirmed empirically: `reqwest`'s
/// own default agent (and other recognizably-library agents) are `403`'d.
const USER_AGENT: &str = "curl/8.7.1";
const PAGE_LIMIT: u32 = 100;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Hard ceiling on pages fetched in one poll — the book is large and unbounded paging would starve
/// this feed's own request budget forever on a market that never runs dry.
const MAX_PAGES: usize = 20;

/// A read-only view of one chain's 1inch Orderbook.
pub struct OneInchApiClient {
    http: reqwest::Client,
    base: String,
    chain: ChainId,
}

impl OneInchApiClient {
    /// `base` is the API root, e.g. `https://api.1inch.dev/orderbook/v4.1`. `api_key` is the
    /// bearer token; the caller owns keeping it out of logs (this client never logs it).
    pub fn new(base: String, chain: ChainId, api_key: String) -> Result<Self, OneInchApiError> {
        let mut headers = reqwest::header::HeaderMap::new();
        let mut auth = reqwest::header::HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|_| OneInchApiError::Build("api key is not a valid header value".into()))?;
        auth.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, auth);
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .timeout(REQUEST_TIMEOUT)
            // A redirect could carry the bearer token to a host we did not choose.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| OneInchApiError::Build(e.to_string()))?;
        Ok(OneInchApiClient {
            http,
            base: base.trim_end_matches('/').to_string(),
            chain,
        })
    }

    /// Every currently open order, across as many pages as the book has (up to [`MAX_PAGES`]).
    pub async fn open_orders(&self) -> Result<Vec<OrderRecord>, OneInchApiError> {
        let mut orders = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut query: Vec<(&str, String)> = vec![("limit", PAGE_LIMIT.to_string())];
            if let Some(c) = &cursor {
                query.push(("cursor", c.clone()));
            }
            let response = self
                .http
                .get(format!("{}/{}/all", self.base, self.chain.0))
                .query(&query)
                .send()
                .await
                .map_err(OneInchApiError::from_reqwest)?;
            let page = match response.status() {
                s if s.is_success() => response
                    .json::<OrderbookPage>()
                    .await
                    .map_err(OneInchApiError::from_reqwest)?,
                s if s.as_u16() == 401 => return Err(OneInchApiError::Unauthorized),
                s if s.as_u16() == 429 => return Err(OneInchApiError::RateLimited),
                s => return Err(OneInchApiError::Status(s.as_u16())),
            };
            let has_more = page.meta.has_more;
            cursor = page.meta.next_cursor;
            orders.extend(page.items);
            if !has_more || cursor.is_none() {
                break;
            }
        }
        Ok(orders)
    }
}

#[derive(Deserialize)]
struct OrderbookPage {
    items: Vec<OrderRecord>,
    meta: Meta,
}

#[derive(Deserialize)]
struct Meta {
    #[serde(rename = "hasMore")]
    has_more: bool,
    #[serde(rename = "nextCursor")]
    next_cursor: Option<String>,
}

/// One order as the API serves it. Only the fields ingest needs are decoded; `data` restates the
/// same order the on-chain `Order`/extension bytes carry, and the normalizer reads those instead
/// so there is one source of truth per order.
#[derive(Clone, Debug, Deserialize)]
pub struct OrderRecord {
    #[serde(rename = "orderHash")]
    pub order_hash: String,
    pub signature: String,
    pub data: OrderData,
    /// Set once the maker no longer has the balance/allowance to honor it, or 1inch itself has
    /// otherwise invalidated it — real, still-open orders leave this `None`.
    #[serde(rename = "orderInvalidReason")]
    pub order_invalid_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct OrderData {
    pub salt: String,
    pub maker: String,
    pub receiver: String,
    #[serde(rename = "makerAsset")]
    pub maker_asset: String,
    #[serde(rename = "takerAsset")]
    pub taker_asset: String,
    #[serde(rename = "makingAmount")]
    pub making_amount: String,
    #[serde(rename = "takingAmount")]
    pub taking_amount: String,
    #[serde(rename = "makerTraits")]
    pub maker_traits: String,
    /// Hex-encoded, `"0x"` when the order carries none.
    pub extension: String,
}

impl OrderRecord {
    /// The feed's output. A field that does not parse is the API contradicting its own schema, so
    /// it is an error rather than an order quietly dropped.
    pub fn to_raw_order(
        &self,
        chain: ChainId,
        observed_at: u64,
    ) -> Result<RawOrder, OneInchApiError> {
        let bad = |field: &'static str| OneInchApiError::Malformed(field);
        let order = super::codec::Order {
            salt: parse_u256(&self.data.salt).ok_or_else(|| bad("salt"))?,
            maker: parse_address(&self.data.maker).ok_or_else(|| bad("maker"))?,
            receiver: parse_address(&self.data.receiver).ok_or_else(|| bad("receiver"))?,
            makerAsset: parse_address(&self.data.maker_asset).ok_or_else(|| bad("makerAsset"))?,
            takerAsset: parse_address(&self.data.taker_asset).ok_or_else(|| bad("takerAsset"))?,
            makingAmount: parse_u256(&self.data.making_amount)
                .ok_or_else(|| bad("makingAmount"))?,
            takingAmount: parse_u256(&self.data.taking_amount)
                .ok_or_else(|| bad("takingAmount"))?,
            makerTraits: parse_u256(&self.data.maker_traits).ok_or_else(|| bad("makerTraits"))?,
        };
        let extension = if self.data.extension == "0x" || self.data.extension.is_empty() {
            Bytes::new()
        } else {
            self.data
                .extension
                .parse::<Bytes>()
                .map_err(|_| bad("extension"))?
        };
        let signature = self
            .signature
            .parse::<Bytes>()
            .map_err(|_| bad("signature"))?;
        let payload = Bytes::from(WireOrder { order, extension }.abi_encode());
        Ok(RawOrder::new(
            ProtocolId::OneInchLimitOrder,
            chain,
            payload,
            signature,
            observed_at,
            OrderSource::OneInch,
        ))
    }
}

/// 1inch serves addresses and amounts as decimal or `0x`-hex strings depending on field and chain;
/// both are accepted since either is unambiguous.
fn parse_address(s: &str) -> Option<Address> {
    s.parse().ok()
}

fn parse_u256(s: &str) -> Option<U256> {
    s.parse().ok()
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OneInchApiError {
    #[error("could not build the orderbook client: {0}")]
    Build(String),
    #[error("orderbook endpoint rejected this client's api key")]
    Unauthorized,
    #[error("orderbook endpoint rate limited this client")]
    RateLimited,
    #[error("orderbook endpoint returned {0}")]
    Status(u16),
    #[error("orderbook endpoint returned an unusable {0}")]
    Malformed(&'static str),
    /// The URL is stripped: it carries no secret, but error text reaches logs and the bearer
    /// header must never be reconstructable from it.
    #[error("orderbook request failed: {0}")]
    Transport(String),
}

impl OneInchApiError {
    fn from_reqwest(error: reqwest::Error) -> OneInchApiError {
        OneInchApiError::Transport(error.without_url().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> OneInchApiClient {
        OneInchApiClient::new(
            "https://api.1inch.dev/orderbook/v4.1/".to_string(),
            ChainId(1),
            "test-key".to_string(),
        )
        .expect("client builds")
    }

    #[test]
    fn trailing_slashes_do_not_double_up() {
        assert_eq!(client().base, "https://api.1inch.dev/orderbook/v4.1");
    }

    #[test]
    fn a_record_becomes_a_raw_order() {
        let record = OrderRecord {
            order_hash: "0xaa".to_string(),
            signature: "0xabcd".to_string(),
            order_invalid_reason: None,
            data: OrderData {
                salt: "1".to_string(),
                maker: "0x1111111111111111111111111111111111111111".to_string(),
                receiver: "0x0000000000000000000000000000000000000000".to_string(),
                maker_asset: "0x2222222222222222222222222222222222222222".to_string(),
                taker_asset: "0x3333333333333333333333333333333333333333".to_string(),
                making_amount: "1000".to_string(),
                taking_amount: "2000".to_string(),
                maker_traits: "0".to_string(),
                extension: "0x".to_string(),
            },
        };
        let raw = record
            .to_raw_order(ChainId(1), 1_788_941_257)
            .expect("converts");
        assert_eq!(raw.protocol, ProtocolId::OneInchLimitOrder);
        assert_eq!(raw.chain, ChainId(1));
        assert_eq!(raw.source, OrderSource::OneInch);
        assert_eq!(raw.observed_at, 1_788_941_257);
        let decoded = WireOrder::abi_decode(&raw.payload).expect("re-decodes");
        assert_eq!(decoded.order.makingAmount, U256::from(1_000u64));
        assert!(decoded.extension.is_empty());
    }

    #[test]
    fn an_extension_carries_through_as_raw_bytes() {
        let mut record_data = OrderData {
            salt: "1".to_string(),
            maker: "0x1111111111111111111111111111111111111111".to_string(),
            receiver: "0x0000000000000000000000000000000000000000".to_string(),
            maker_asset: "0x2222222222222222222222222222222222222222".to_string(),
            taker_asset: "0x3333333333333333333333333333333333333333".to_string(),
            making_amount: "1000".to_string(),
            taking_amount: "2000".to_string(),
            maker_traits: "0".to_string(),
            extension: "0xdeadbeef".to_string(),
        };
        record_data.extension = "0xdeadbeef".to_string();
        let record = OrderRecord {
            order_hash: "0xaa".to_string(),
            signature: "0xabcd".to_string(),
            order_invalid_reason: None,
            data: record_data,
        };
        let raw = record.to_raw_order(ChainId(1), 0).expect("converts");
        let decoded = WireOrder::abi_decode(&raw.payload).expect("re-decodes");
        assert_eq!(decoded.extension.as_ref(), &[0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn unusable_amount_is_an_error_not_a_silent_drop() {
        let record = OrderRecord {
            order_hash: "0xaa".to_string(),
            signature: "0xabcd".to_string(),
            order_invalid_reason: None,
            data: OrderData {
                salt: "1".to_string(),
                maker: "0x1111111111111111111111111111111111111111".to_string(),
                receiver: "0x0000000000000000000000000000000000000000".to_string(),
                maker_asset: "0x2222222222222222222222222222222222222222".to_string(),
                taker_asset: "0x3333333333333333333333333333333333333333".to_string(),
                making_amount: "not a number".to_string(),
                taking_amount: "2000".to_string(),
                maker_traits: "0".to_string(),
                extension: "0x".to_string(),
            },
        };
        assert!(matches!(
            record.to_raw_order(ChainId(1), 0),
            Err(OneInchApiError::Malformed("makingAmount"))
        ));
    }

    #[test]
    fn an_empty_page_deserializes() {
        let page: OrderbookPage =
            serde_json::from_str(r#"{"items":[],"meta":{"hasMore":false,"nextCursor":null}}"#)
                .expect("parses");
        assert!(page.items.is_empty());
        assert!(!page.meta.has_more);
    }

    #[test]
    fn a_live_shaped_record_deserializes() {
        let body = r#"{"items":[{
            "orderHash":"0xf01091d3",
            "signature":"0x2233",
            "orderInvalidReason":null,
            "makerBalance":"1000000",
            "makerAllowance":"1000000",
            "remainingMakerAmount":"1000000",
            "makerRate":"1",
            "takerRate":"1",
            "isMakerContract":false,
            "createDateTime":"2026-01-01T00:00:00.000Z",
            "data":{
                "salt":"123",
                "maker":"0x1111111111111111111111111111111111111111",
                "receiver":"0x0000000000000000000000000000000000000000",
                "makerAsset":"0x2222222222222222222222222222222222222222",
                "takerAsset":"0x3333333333333333333333333333333333333333",
                "makingAmount":"1000000",
                "takingAmount":"7600000000",
                "makerTraits":"0",
                "extension":"0x"
            }
        }],"meta":{"hasMore":false,"nextCursor":null}}"#;
        let page: OrderbookPage =
            serde_json::from_str(body).expect("ignores the fields we do not read");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].order_hash, "0xf01091d3");
    }
}
