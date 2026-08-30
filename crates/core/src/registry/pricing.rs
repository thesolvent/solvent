//! Off-chain pricing: a decoded maker strategy + a taker's direction and amount
//! become the exact amount the on-chain `quote()` would return, or a reason it
//! can't be priced. Zero I/O — reads only the folded snapshot's state.

use alloy_primitives::{Address, U256};
use thiserror::Error;

use super::curves::{
    apply_flat_fee_in, apply_flat_fee_out, ConcentratePool, CurveError, PeggedPool, Pricing,
    XycPool,
};
use crate::primitives::registry::{Curve, CurveSpec, MakerStrategy};

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

/// Price a taker trading `amount` in the `token_in -> token_out` direction (`exact_in` returns the
/// output, else the required input). Prices whatever `strategy` it is handed — callers route only
/// active ones.
pub fn price(
    strategy: &MakerStrategy,
    token_in: Address,
    token_out: Address,
    amount: U256,
    exact_in: bool,
) -> Result<U256, PriceError> {
    let CurveSpec::Priceable { curve, fees_in_bps } = &strategy.curve else {
        return Err(PriceError::Unsupported);
    };
    let balance_in = strategy.balance(&token_in);
    let balance_out = strategy.balance(&token_out);
    let pool: Box<dyn Pricing> = match curve {
        Curve::Xyc => Box::new(XycPool::from_reserves(balance_in, balance_out)),
        Curve::Concentrate {
            sqrt_price_min,
            sqrt_price_max,
        } => Box::new(ConcentratePool::from_reserves_and_bounds(
            token_in,
            token_out,
            balance_in,
            balance_out,
            *sqrt_price_min,
            *sqrt_price_max,
        )),
        Curve::Pegged(params) => Box::new(PeggedPool::from_reserves_and_params(
            token_in,
            token_out,
            balance_in,
            balance_out,
            *params,
        )),
    };
    let quoted = if exact_in {
        // Flat fees shrink the input in program order, then the curve runs.
        let net_in = fees_in_bps
            .iter()
            .try_fold(amount, |a, &bps| apply_flat_fee_in(a, bps))?;
        pool.quote_exact_in(net_in)?
    } else {
        // The curve runs, then flat fees gross the input up in reverse order.
        let curve_in = pool.quote_exact_out(amount)?;
        fees_in_bps
            .iter()
            .rev()
            .try_fold(curve_in, |a, &bps| apply_flat_fee_out(a, bps))?
    };
    Ok(quoted)
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
    fn spec(curve: Curve, fees_in_bps: &[u32]) -> CurveSpec {
        CurveSpec::Priceable {
            curve,
            fees_in_bps: fees_in_bps.to_vec(),
        }
    }

    #[test]
    fn xyc_price_matches_the_pool() {
        let s = strategy(
            spec(Curve::Xyc, &[]),
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
            spec(Curve::Pegged(params), &[]),
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
    fn flat_fee_reduces_output_and_grosses_input() {
        let fee = 3_000_000u32; // 0.3% at 1e9 BPS
        let s = strategy(
            spec(Curve::Xyc, &[fee]),
            tok(1),
            e18(1000),
            tok(2),
            e18(1000),
        );
        let pool = XycPool::from_reserves(e18(1000), e18(1000));

        // exact-in: the fee shrinks the curve's input.
        let got_in = price(&s, tok(1), tok(2), e18(100), true).unwrap();
        assert_eq!(
            got_in,
            pool.quote_exact_in(apply_flat_fee_in(e18(100), fee).unwrap())
                .unwrap()
        );
        assert!(got_in < pool.quote_exact_in(e18(100)).unwrap());

        // exact-out: the fee grosses the curve's input up.
        let got_out = price(&s, tok(1), tok(2), e18(90), false).unwrap();
        let curve_in = pool.quote_exact_out(e18(90)).unwrap();
        assert_eq!(got_out, apply_flat_fee_out(curve_in, fee).unwrap());
        assert!(got_out > curve_in);
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
            spec(Curve::Xyc, &[]),
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
