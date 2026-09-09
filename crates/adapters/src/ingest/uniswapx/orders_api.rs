//! A read-only client for Uniswap's Orders API — the only place an unfilled UniswapX order exists.
//! Orders are off-chain signed messages until someone fills them: there is no mempool to watch and
//! no event to subscribe to, so polling this endpoint is the sole discovery channel, for us and for
//! every other filler.
//!
//! The endpoint's constraints are unusual enough to be worth stating, since each one was found the
//! hard way and each shapes the code below:
//!
//! - **No authentication**, but the edge rejects unfamiliar user agents with `403`. A missing
//!   `User-Agent` looks exactly like an auth failure, so [`OrdersApiError`] separates the two.
//! - **No pagination.** `cursor` is a `400`, and so is any `sort`/`desc` other than the default.
//!   One page of the newest orders is all there is; `limit` silently clamps to 50.
//! - **`pair` is documented but returns `500`.** Pair filtering happens on our side.
//! - **4 requests per second.** The caller owns that budget (see `HostedFeed`).

use std::time::Duration;

use alloy::primitives::{Address, Bytes};
use serde::Deserialize;
use thiserror::Error;

use solvent_core::primitives::ingest::{ProtocolId, RawOrder};
use solvent_core::primitives::ChainId;

/// Identifies this filler to the API's edge. Absent or unfamiliar agents are refused with `403`.
const USER_AGENT: &str = concat!("solvent-filler/", env!("CARGO_PKG_VERSION"));
/// The endpoint clamps to this; asking for more is silently reduced, so ask for exactly it.
const PAGE_LIMIT: u32 = 50;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Which slice of the book to fetch. Both are `orderStatus=open`; they differ in whether the query
/// is narrowed to orders already assigned to us.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every open order on the chain.
    Book,
    /// Only orders naming `filler` as their exclusive filler — the ones we may fill at face value
    /// and, if we ever hold a quoter seat, the ones we are on the hook for.
    ExclusiveTo(Address),
}

/// A read-only view of the Orders API for one chain and order type.
pub struct OrdersApiClient {
    http: reqwest::Client,
    base: String,
    chain: ChainId,
    order_type: String,
}

impl OrdersApiClient {
    /// `base` is the API root, e.g. `https://api.uniswap.org/v2` — or a local mirror serving the
    /// same shape, which is how the fork harness feeds this client without touching the network.
    pub fn new(base: String, chain: ChainId, order_type: String) -> Result<Self, OrdersApiError> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            // A redirect could carry our headers to a host we did not choose.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| OrdersApiError::Build(e.to_string()))?;
        Ok(OrdersApiClient {
            http,
            base: base.trim_end_matches('/').to_string(),
            chain,
            order_type,
        })
    }

    /// One page of the newest open orders. There is no "next page" — the caller re-polls and dedups.
    pub async fn open_orders(&self, scope: Scope) -> Result<Vec<OrderRecord>, OrdersApiError> {
        let mut query: Vec<(&str, String)> = vec![
            ("chainId", self.chain.0.to_string()),
            ("orderStatus", "open".to_string()),
            ("orderType", self.order_type.clone()),
            ("limit", PAGE_LIMIT.to_string()),
        ];
        if let Scope::ExclusiveTo(filler) = scope {
            query.push(("filler", filler.to_string()));
        }

        let response = self
            .http
            .get(format!("{}/orders", self.base))
            .query(&query)
            .send()
            .await
            .map_err(OrdersApiError::from_reqwest)?;

        match response.status() {
            s if s.is_success() => Ok(response
                .json::<OrdersPage>()
                .await
                .map_err(OrdersApiError::from_reqwest)?
                .orders),
            // Not a rate limit and not transient: the edge does not recognise us, and retrying
            // unchanged will keep failing. Callers surface it rather than backing off.
            s if s.as_u16() == 403 => Err(OrdersApiError::Refused),
            s if s.as_u16() == 429 => Err(OrdersApiError::RateLimited),
            s => Err(OrdersApiError::Status(s.as_u16())),
        }
    }
}

#[derive(Deserialize)]
struct OrdersPage {
    orders: Vec<OrderRecord>,
}

/// One order as the API serves it. Only the fields ingest needs are decoded; the rest of the
/// record (decoded amounts, status, quote ids) restates what `encoded_order` already contains, and
/// the normalizer reads that instead so there is one source of truth per order.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderRecord {
    pub order_hash: String,
    pub encoded_order: String,
    pub signature: String,
    pub created_at: u64,
}

impl OrderRecord {
    /// The feed's output. Hex that does not parse is the API contradicting itself, so it is an
    /// error rather than an order we quietly drop.
    pub fn to_raw_order(&self, chain: ChainId) -> Result<RawOrder, OrdersApiError> {
        let payload = self
            .encoded_order
            .parse::<Bytes>()
            .map_err(|_| OrdersApiError::Malformed("encodedOrder"))?;
        let signature = self
            .signature
            .parse::<Bytes>()
            .map_err(|_| OrdersApiError::Malformed("signature"))?;
        Ok(RawOrder::new(
            ProtocolId::UniswapXV2,
            chain,
            payload,
            signature,
            self.created_at,
        ))
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OrdersApiError {
    #[error("could not build the orders client: {0}")]
    Build(String),
    /// The edge refused us outright — a user-agent or IP policy, not a transient fault. Backing off
    /// will not help; the feed treats it as an outage.
    #[error("orders endpoint refused this client")]
    Refused,
    #[error("orders endpoint rate limited this client")]
    RateLimited,
    #[error("orders endpoint returned {0}")]
    Status(u16),
    #[error("orders endpoint returned an unusable {0}")]
    Malformed(&'static str),
    /// The URL is stripped: it carries query parameters, and error text reaches logs.
    #[error("orders request failed: {0}")]
    Transport(String),
}

impl OrdersApiError {
    fn from_reqwest(error: reqwest::Error) -> OrdersApiError {
        OrdersApiError::Transport(error.without_url().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    fn client(base: &str) -> OrdersApiClient {
        OrdersApiClient::new(base.to_string(), ChainId(1), "Dutch_V2".to_string())
            .expect("client builds")
    }

    #[test]
    fn trailing_slashes_do_not_double_up() {
        assert_eq!(
            client("https://api.uniswap.org/v2/").base,
            client("https://api.uniswap.org/v2").base
        );
    }

    /// The record the API serves becomes the raw order ingest consumes, hex intact.
    #[test]
    fn a_record_becomes_a_raw_order() {
        let record = OrderRecord {
            order_hash: "0xaa".to_string(),
            encoded_order: "0x1234".to_string(),
            signature: "0xabcd".to_string(),
            created_at: 1788941257,
        };
        let raw = record.to_raw_order(ChainId(1)).expect("converts");
        assert_eq!(raw.protocol, ProtocolId::UniswapXV2);
        assert_eq!(raw.chain, ChainId(1));
        assert_eq!(raw.payload, "0x1234".parse::<Bytes>().expect("hex"));
        assert_eq!(raw.signature, "0xabcd".parse::<Bytes>().expect("hex"));
        assert_eq!(raw.observed_at, 1788941257);
    }

    #[test]
    fn unusable_hex_is_an_error_not_a_silent_drop() {
        let record = OrderRecord {
            order_hash: "0xaa".to_string(),
            encoded_order: "not hex".to_string(),
            signature: "0xabcd".to_string(),
            created_at: 0,
        };
        assert!(matches!(
            record.to_raw_order(ChainId(1)),
            Err(OrdersApiError::Malformed("encodedOrder"))
        ));
    }

    /// The page wrapper is what the endpoint actually returns; an empty book is normal, not an error.
    #[test]
    fn an_empty_book_deserializes() {
        let page: OrdersPage = serde_json::from_str(r#"{"orders":[]}"#).expect("parses");
        assert!(page.orders.is_empty());
    }

    #[test]
    fn a_live_shaped_page_deserializes() {
        let body = r#"{"orders":[{
            "orderHash":"0x203583e457e5",
            "encodedOrder":"0x0011",
            "signature":"0x2233",
            "createdAt":1788941257,
            "orderStatus":"open",
            "type":"Dutch_V2",
            "chainId":1,
            "input":{"token":"0x00","startAmount":"1","endAmount":"1"},
            "outputs":[]
        }]}"#;
        let page: OrdersPage =
            serde_json::from_str(body).expect("ignores the fields we do not read");
        assert_eq!(page.orders.len(), 1);
        assert_eq!(page.orders[0].created_at, 1788941257);
    }

    #[test]
    fn scope_narrows_to_our_filler() {
        let filler = address!("1111111111111111111111111111111111111111");
        assert_ne!(Scope::Book, Scope::ExclusiveTo(filler));
    }
}
