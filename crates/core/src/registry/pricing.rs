//! Off-chain pricing: a decoded maker strategy + a taker's direction and amount
//! become the exact amount the on-chain `quote()` would return, or a reason it
//! can't be priced. Zero I/O — reads only the folded snapshot's state.

use alloy_primitives::{Address, U256};
use thiserror::Error;

use super::curves::{ConcentratePool, CurveError, PeggedPool, Pricing, XycPool};
use crate::primitives::registry::{CurveSpec, MakerStrategy};

/// Why a strategy could not be priced for a request.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum PriceError {
    /// The program is not a priceable Aqua curve.
    #[error("strategy uses an unsupported curve")]
    Unsupported,
    /// The curve rejected the trade (empty reserves, amount too large, overflow…).
    #[error(transparent)]
    Curve(#[from] CurveError),
}

/// Price a taker trading `amount` on `strategy` in the `token_in -> token_out`
/// direction: `exact_in` returns the output, otherwise the required input. The
/// right curve pool is built from the decoded curve + current virtual reserves,
/// oriented by the token addresses.
pub fn price(
    strategy: &MakerStrategy,
    token_in: Address,
    token_out: Address,
    amount: U256,
    exact_in: bool,
) -> Result<U256, PriceError> {
    let balance_in = strategy.balance(&token_in);
    let balance_out = strategy.balance(&token_out);
    let quote = |pool: &dyn Pricing| {
        if exact_in {
            pool.quote_exact_in(amount)
        } else {
            pool.quote_exact_out(amount)
        }
    };
    let amount = match &strategy.curve {
        CurveSpec::Xyc => quote(&XycPool::from_reserves(balance_in, balance_out)),
        CurveSpec::Concentrate {
            sqrt_price_min,
            sqrt_price_max,
        } => quote(&ConcentratePool::from_reserves_and_bounds(
            token_in,
            token_out,
            balance_in,
            balance_out,
            *sqrt_price_min,
            *sqrt_price_max,
        )),
        CurveSpec::Pegged(params) => quote(&PeggedPool::from_reserves_and_params(
            token_in,
            token_out,
            balance_in,
            balance_out,
            *params,
        )),
        CurveSpec::Unsupported => return Err(PriceError::Unsupported),
    };
    Ok(amount?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::registry::{PeggedParams, StrategyKey};
    use crate::primitives::{MakerId, StrategyHash};
    use std::collections::BTreeMap;

    fn tok(n: u8) -> Address {
        Address::from([n; 20])
    }

    fn strategy(
        curve: CurveSpec,
        in_tok: Address,
        in_bal: U256,
        out_tok: Address,
        out_bal: U256,
    ) -> MakerStrategy {
        let mut balances = BTreeMap::new();
        balances.insert(in_tok, in_bal);
        balances.insert(out_tok, out_bal);
        MakerStrategy {
            key: StrategyKey {
                maker: MakerId(Address::ZERO),
                app: Address::ZERO,
                strategy_hash: StrategyHash(Default::default()),
            },
            curve,
            balances,
            active: true,
        }
    }

    fn e18(n: u128) -> U256 {
        U256::from(n) * U256::from(1_000_000_000_000_000_000u128)
    }

    #[test]
    fn xyc_price_matches_the_pool() {
        let s = strategy(
            CurveSpec::Xyc,
            tok(1),
            U256::from(1000u64),
            tok(2),
            U256::from(1000u64),
        );
        let got = price(&s, tok(1), tok(2), U256::from(100u64), true).unwrap();
        let pool = XycPool::from_reserves(U256::from(1000u64), U256::from(1000u64));
        assert_eq!(got, pool.quote_exact_in(U256::from(100u64)).unwrap());
    }

    #[test]
    fn pegged_price_orients_by_token_address() {
        // 18/18 stable pool: rates 1, x0 = y0 = reserve.
        let params = PeggedParams {
            x0: e18(1000),
            y0: e18(1000),
            linear_width: U256::from(100u64) * U256::from(10u64).pow(U256::from(27u64)),
            rate_lt: U256::from(1u64),
            rate_gt: U256::from(1u64),
        };
        let s = strategy(
            CurveSpec::Pegged(params),
            tok(1),
            e18(1000),
            tok(2),
            e18(1000),
        );
        // Selling the lower-address token into a stable pool beats constant-product.
        let peg = price(&s, tok(1), tok(2), e18(100), true).unwrap();
        let xyc = XycPool::from_reserves(e18(1000), e18(1000))
            .quote_exact_in(e18(100))
            .unwrap();
        assert!(peg > xyc);
    }

    #[test]
    fn unsupported_and_empty_reserves_error() {
        let s = strategy(
            CurveSpec::Unsupported,
            tok(1),
            U256::from(1000u64),
            tok(2),
            U256::from(1000u64),
        );
        assert_eq!(
            price(&s, tok(1), tok(2), U256::from(1u64), true),
            Err(PriceError::Unsupported)
        );

        let s = strategy(
            CurveSpec::Xyc,
            tok(1),
            U256::from(1000u64),
            tok(2),
            U256::from(1000u64),
        );
        // Token 3 isn't in the strategy -> zero reserve -> curve rejects.
        assert!(matches!(
            price(&s, tok(1), tok(3), U256::from(1u64), true),
            Err(PriceError::Curve(_))
        ));
    }
}
