//! Differential test: the closed-form curve ports vs. real on-chain `quote()`.
//!
//! Corpus: `tests/fixtures/aqua_curve_vectors.json`, generated against
//! source-deployed Aqua + AquaSwapVMRouter v1.0.1 on anvil (see the study lab's
//! `swapvm-lab/src/gen-corpus.ts`). Each vector pins a real `quote()` result
//! (value or revert) for a boundary-heavy grid of
//! `(direction x exactIn/exactOut x amount)`; the port must reproduce it exactly.

use alloy_primitives::{Address, U256};
use serde::Deserialize;
use solvent_core::primitives::registry::PeggedParams;
use solvent_core::registry::{
    apply_flat_fee_in, apply_flat_fee_out, ConcentratePool, CurveError, PeggedPool, Pricing,
    XycPool,
};

#[derive(Deserialize)]
struct Corpus {
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Vector {
    curve: String,
    token_in_is_lt: bool,
    balance_in: String,
    balance_out: String,
    exact_in: bool,
    amount: String,
    fee_bps: u32,
    sqrt_price_min: Option<String>,
    sqrt_price_max: Option<String>,
    x0: Option<String>,
    y0: Option<String>,
    linear_width: Option<String>,
    rate_lt: Option<String>,
    rate_gt: Option<String>,
    revert: bool,
    amount_in: Option<String>,
    amount_out: Option<String>,
}

fn u(s: &str) -> U256 {
    U256::from_str_radix(s, 10).expect("decimal U256")
}

// Two addresses with lo < hi, so `token_in_is_lt` maps to the on-chain ordering.
fn tokens(in_is_lt: bool) -> (Address, Address) {
    let lo = Address::from([0x11u8; 20]);
    let hi = Address::from([0x22u8; 20]);
    if in_is_lt {
        (lo, hi)
    } else {
        (hi, lo)
    }
}

fn quote_vector(v: &Vector) -> Result<U256, CurveError> {
    let (balance_in, balance_out) = (u(&v.balance_in), u(&v.balance_out));
    let amount = u(&v.amount);
    let (token_in, token_out) = tokens(v.token_in_is_lt);
    // A flat fee shrinks the curve's input (exact-in) or grosses it up (exact-out).
    let run = |p: &dyn Pricing| -> Result<U256, CurveError> {
        if v.exact_in {
            let net_in = if v.fee_bps > 0 {
                apply_flat_fee_in(amount, v.fee_bps)?
            } else {
                amount
            };
            p.quote_exact_in(net_in)
        } else {
            let curve_in = p.quote_exact_out(amount)?;
            if v.fee_bps > 0 {
                apply_flat_fee_out(curve_in, v.fee_bps)
            } else {
                Ok(curve_in)
            }
        }
    };
    match v.curve.as_str() {
        "xyc" => run(&XycPool::from_reserves(balance_in, balance_out)),
        "concentrate" => run(&ConcentratePool::from_reserves_and_bounds(
            token_in,
            token_out,
            balance_in,
            balance_out,
            u(v.sqrt_price_min.as_deref().expect("sqrtMin")),
            u(v.sqrt_price_max.as_deref().expect("sqrtMax")),
        )),
        "pegged" => run(&PeggedPool::from_reserves_and_params(
            token_in,
            token_out,
            balance_in,
            balance_out,
            PeggedParams {
                x0: u(v.x0.as_deref().expect("x0")),
                y0: u(v.y0.as_deref().expect("y0")),
                linear_width: u(v.linear_width.as_deref().expect("A")),
                rate_lt: u(v.rate_lt.as_deref().expect("rateLt")),
                rate_gt: u(v.rate_gt.as_deref().expect("rateGt")),
            },
        )),
        other => panic!("unknown curve {other}"),
    }
}

#[test]
fn differential_against_onchain_quote() {
    let corpus: Corpus = serde_json::from_str(include_str!("fixtures/aqua_curve_vectors.json"))
        .expect("parse corpus");
    assert!(corpus.vectors.len() >= 200, "corpus too small");

    for (i, v) in corpus.vectors.iter().enumerate() {
        let got = quote_vector(v);
        if v.revert {
            // The contract reverts on curve-math reverts and on a zero-output quote
            // (a router-level guard, above the curve). Both are acceptable parity.
            let zero_output = matches!(got, Ok(x) if x.is_zero());
            assert!(
                got.is_err() || zero_output,
                "vector {i} ({}): on-chain reverted but port returned {got:?}",
                v.curve
            );
        } else {
            let expected = if v.exact_in {
                u(v.amount_out.as_deref().expect("amountOut"))
            } else {
                u(v.amount_in.as_deref().expect("amountIn"))
            };
            assert_eq!(
                got,
                Ok(expected),
                "vector {i} ({}, exactIn={}): port disagrees with on-chain quote()",
                v.curve,
                v.exact_in
            );
        }
    }
}
