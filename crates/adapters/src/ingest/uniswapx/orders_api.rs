//! Read-only access to UniswapX's public Dutch-V2 order book.

use std::time::Duration;

use alloy::primitives::Bytes;
use serde::Deserialize;
use thiserror::Error;

use solvent_core::primitives::ingest::{ProtocolId, RawOrder};
use solvent_core::primitives::ChainId;

const MAINNET_ORDERS_API: &str = "https://api.uniswap.org/v2";
const MAINNET: ChainId = ChainId(1);
const ORDER_TYPE: &str = "Dutch_V2";
const PAGE_LIMIT: u32 = 50;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const USER_AGENT: &str = concat!("solvent-feed/", env!("CARGO_PKG_VERSION"));

/// A client for the one page of newest open mainnet Dutch-V2 orders the public API exposes.
pub struct OrdersApiClient {
    http: reqwest::Client,
}

impl OrdersApiClient {
    pub fn mainnet() -> Result<Self, OrdersApiError> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| OrdersApiError::Build(error.to_string()))?;
        Ok(Self { http })
    }

    /// Fetch the API's bounded newest-order page. The endpoint has no pagination, so callers
    /// re-poll it at the published request limit and deduplicate locally.
    pub async fn open_orders(&self) -> Result<Vec<OrderRecord>, OrdersApiError> {
        let response = self
            .http
            .get(format!("{MAINNET_ORDERS_API}/orders"))
            .query(&[
                ("chainId", MAINNET.0.to_string()),
                ("orderStatus", "open".to_string()),
                ("orderType", ORDER_TYPE.to_string()),
                ("limit", PAGE_LIMIT.to_string()),
            ])
            .send()
            .await
            .map_err(OrdersApiError::transport)?;
        if !response.status().is_success() {
            return Err(OrdersApiError::Status(response.status().as_u16()));
        }
        response
            .json::<OrdersPage>()
            .await
            .map(|page| page.orders)
            .map_err(OrdersApiError::transport)
    }
}

#[derive(Deserialize)]
struct OrdersPage {
    orders: Vec<OrderRecord>,
}

/// The API fields needed to turn a public order into the existing UniswapX wire format.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderRecord {
    encoded_order: String,
    signature: String,
}

impl OrderRecord {
    pub(crate) fn raw(&self, observed_at: u64) -> Result<RawOrder, OrdersApiError> {
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
            MAINNET,
            payload,
            signature,
            observed_at,
        ))
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OrdersApiError {
    #[error("could not build the UniswapX Orders API client: {0}")]
    Build(String),
    #[error("UniswapX Orders API returned HTTP {0}")]
    Status(u16),
    #[error("UniswapX Orders API returned an unusable {0}")]
    Malformed(&'static str),
    #[error("UniswapX Orders API request failed: {0}")]
    Transport(String),
}

impl OrdersApiError {
    fn transport(error: reqwest::Error) -> Self {
        Self::Transport(error.without_url().to_string())
    }
}
