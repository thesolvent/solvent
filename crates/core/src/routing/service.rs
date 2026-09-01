//! The router's public entry: turn an intent into a reservable `RoutePlan`, or decline.

use alloy_primitives::{Address, U256};

use crate::deps::routing::{GasPrice, PriceOracle};
use crate::ledger::AvailableSnapshot;
use crate::obs::warn;
use crate::primitives::pricing::Ratio;
use crate::primitives::registry::Snapshot;
use crate::primitives::routing::gas::per_leg_cost as compute_leg_cost;
use crate::primitives::routing::{RoutePlan, RouteRequest, RoutingConfig};

use super::{select, solve_sparse};

/// Resolve the per-leg gas cost in the **spread token**'s base units from the live cache — gas
/// price and the native + spread-token USD prices. The spread token is `token_out` for exact-in
/// and `token_in` for exact-out (the token the resolver's profit is measured in), so the caller
/// passes whichever the direction needs. `None` if any input is unavailable (the caller then
/// routes without a gas threshold). Off the quote hot path; the ports read a cache.
pub async fn resolve_leg_cost(
    gas: &dyn GasPrice,
    oracle: &dyn PriceOracle,
    gas_units: u64,
    native: Address,
    spread_token: Address,
    spread_token_decimals: u8,
) -> Option<U256> {
    let gas_wei = gas.gas_price_wei().await.ok()?;
    let native_price = oracle.price(native).await.ok()?;
    let token_price = oracle.price(spread_token).await.ok()?;
    Some(compute_leg_cost(
        gas_units,
        gas_wei,
        native_price,
        token_price,
        spread_token_decimals,
    ))
}

/// Route `request`: rank candidates ([`select`]), solve the gas-sparse split
/// ([`solve_sparse`]), and gate on the taker's bound — `min_out` for exact-in, `max_in` for
/// exact-out. `per_leg_cost` is the resolved per-leg gas in the spread token (output for
/// exact-in, input for exact-out); `warm` is the last λ for this pair. Returns `None` when
/// there is no profitable, reservable route — the spread is a checked subtraction, so a bound
/// the split can't beat declines the plan.
pub fn route(
    snapshot: &Snapshot,
    caps: &AvailableSnapshot,
    request: &RouteRequest,
    bound: U256,
    config: &RoutingConfig,
    per_leg_cost: U256,
    warm: Option<&Ratio>,
) -> Option<RoutePlan> {
    let selection = select(snapshot, caps, request, config.max_candidates);
    let split = solve_sparse(
        &selection.chosen,
        request,
        per_leg_cost,
        config.max_legs,
        warm,
    )?;
    // Certificate diagnostic: a dropped pool whose (conservatively-estimated) spot marginal
    // exceeds the optimum's water level λ* signals the funnel `k` was likely too small.
    if selection
        .best_omitted_spot
        .is_some_and(|spot| spot > split.lambda)
    {
        warn!(intent = %request.intent, "routing funnel too small: an omitted pool's spot exceeds the optimum's marginal price");
    }
    // The resolver's spread over the taker's bound, both directions charging gas.
    let expected_profit = if request.exact_in {
        split.net_output(per_leg_cost).checked_sub(bound)? // net output clears `min_out`
    } else {
        bound.checked_sub(split.gross_input(per_leg_cost))? // input + gas stays under `max_in`
    };
    Some(RoutePlan {
        intent: request.intent,
        legs: split.legs,
        expected_profit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::ledger::AccountKey;
    use crate::primitives::registry::{Curve, CurveSpec, MakerStrategy, StrategyKey};
    use crate::primitives::{IntentId, MakerId, StrategyHash};
    use alloy_primitives::{Address, B256};
    use std::collections::BTreeMap;

    fn tok(n: u8) -> Address {
        Address::from([n; 20])
    }

    /// One ample XYC pool over `(tok(1), tok(2))`, with slack caps.
    fn fixture() -> (Snapshot, AvailableSnapshot, RouteRequest) {
        let maker = MakerId(Address::from([1u8; 20]));
        let hash = StrategyHash(B256::from([1u8; 32]));
        let mut balances = BTreeMap::new();
        balances.insert(tok(1), U256::from(10_000u64));
        balances.insert(tok(2), U256::from(10_000u64));
        let strategy = MakerStrategy {
            key: StrategyKey {
                maker,
                app: Address::ZERO,
                strategy_hash: hash,
            },
            curve: CurveSpec::Priceable {
                curve: Curve::Xyc,
                fees_in_bps: vec![],
            },
            balances,
            active: true,
        };
        let snapshot = Snapshot::from_strategies([strategy]);
        let caps = AvailableSnapshot(BTreeMap::from([
            (
                AccountKey::WalletBudget {
                    maker,
                    token: tok(2),
                },
                U256::from(10_000u64),
            ),
            (
                AccountKey::StrategyVirtual {
                    maker,
                    strategy_hash: hash,
                    token: tok(2),
                },
                U256::from(10_000u64),
            ),
        ]));
        let request = RouteRequest {
            intent: IntentId(B256::ZERO),
            token_in: tok(1),
            token_out: tok(2),
            amount: U256::from(100u64),
            exact_in: true,
        };
        (snapshot, caps, request)
    }

    #[test]
    fn routes_when_output_clears_the_min_and_declines_otherwise() {
        let (snap, caps, req) = fixture();
        let cfg = RoutingConfig::new(64, 8, 150_000);
        // ~90 out for 100 in; a min-out below that yields a plan with the surplus as profit.
        let plan = route(
            &snap,
            &caps,
            &req,
            U256::from(50u64),
            &cfg,
            U256::ZERO,
            None,
        )
        .unwrap();
        assert_eq!(plan.legs.len(), 1);
        assert!(plan.expected_profit > U256::ZERO);
        // A min-out above the achievable output declines the route.
        assert!(route(
            &snap,
            &caps,
            &req,
            U256::from(200u64),
            &cfg,
            U256::ZERO,
            None
        )
        .is_none());
    }

    #[test]
    fn exact_out_spread_charges_gas() {
        let (snap, caps, mut req) = fixture();
        req.exact_in = false; // deliver `amount` of token_out for at most `max_in`
        let cfg = RoutingConfig::new(64, 8, 150_000);
        // ~102 in for 100 out; a generous max_in with no gas leaves surplus.
        let plan = route(
            &snap,
            &caps,
            &req,
            U256::from(200u64),
            &cfg,
            U256::ZERO,
            None,
        )
        .unwrap();
        assert!(plan.expected_profit > U256::ZERO);
        // The same max_in, but per-leg gas that swallows the surplus, declines the route.
        assert!(route(
            &snap,
            &caps,
            &req,
            U256::from(200u64),
            &cfg,
            U256::from(150u64),
            None
        )
        .is_none());
    }

    struct FakeGas(u128);
    #[async_trait::async_trait]
    impl GasPrice for FakeGas {
        async fn gas_price_wei(&self) -> Result<u128, crate::deps::routing::GasPriceError> {
            Ok(self.0)
        }
    }

    struct FakeOracle(std::collections::HashMap<Address, crate::primitives::UsdPrice>);
    #[async_trait::async_trait]
    impl PriceOracle for FakeOracle {
        async fn price(
            &self,
            token: Address,
        ) -> Result<crate::primitives::UsdPrice, crate::deps::routing::PriceOracleError> {
            self.0
                .get(&token)
                .copied()
                .ok_or(crate::deps::routing::PriceOracleError::NotFound(token))
        }
    }

    #[tokio::test]
    async fn resolves_leg_cost_from_the_ports() {
        use crate::primitives::UsdPrice;
        use rust_decimal::Decimal;
        let (native, token) = (tok(9), tok(2));
        let gas = FakeGas(20_000_000_000);
        let oracle = FakeOracle(std::collections::HashMap::from([
            (native, UsdPrice(Decimal::from(3000u32))),
            (token, UsdPrice(Decimal::from(1u32))),
        ]));
        // 150k × 20 gwei = 0.003 native; native $3000 ⇒ $9; token $1, 18 dp ⇒ 9e18.
        let cost = resolve_leg_cost(&gas, &oracle, 150_000, native, token, 18)
            .await
            .unwrap();
        assert_eq!(
            cost,
            U256::from(9u64) * U256::from(10u64).pow(U256::from(18u64))
        );
    }
}
