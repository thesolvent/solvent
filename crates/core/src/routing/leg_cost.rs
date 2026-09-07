//! Per-leg gas cost for a route, resolved from the live market cache. Shared by the quote and
//! swap paths so both charge routing the same gas.

use std::sync::Arc;

use alloy_primitives::{Address, U256};

use crate::asset::AssetManager;
use crate::deps::routing::{GasPrice, PriceOracle};
use crate::primitives::routing::RouteRequest;

use super::service::resolve_leg_cost;

/// Resolves the per-leg gas cost in a request's spread token from the gas/price cache. Holds the
/// gas and price ports, the catalog (for the spread token's decimals), and the chain's native
/// token and per-leg gas units.
pub struct LegCostResolver {
    gas: Arc<dyn GasPrice>,
    oracle: Arc<dyn PriceOracle>,
    assets: Arc<AssetManager>,
    native: Address,
    gas_units_per_leg: u64,
}

impl LegCostResolver {
    pub fn new(
        gas: Arc<dyn GasPrice>,
        oracle: Arc<dyn PriceOracle>,
        assets: Arc<AssetManager>,
        native: Address,
        gas_units_per_leg: u64,
    ) -> Self {
        Self {
            gas,
            oracle,
            assets,
            native,
            gas_units_per_leg,
        }
    }

    /// Per-leg gas in the request's spread token (output for exact-in, input for exact-out), 0 if
    /// unpriced.
    pub async fn for_request(&self, req: &RouteRequest) -> U256 {
        let spread = if req.exact_in {
            req.token_out
        } else {
            req.token_in
        };
        resolve_leg_cost(
            self.gas.as_ref(),
            self.oracle.as_ref(),
            self.gas_units_per_leg,
            self.native,
            spread,
            self.assets.decimals(&spread),
        )
        .await
        .unwrap_or(U256::ZERO)
    }
}
