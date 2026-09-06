//! Maker LVR recapture: split a confirmed fill's realized spread back to the makers whose pools it
//! rebalanced. Pure and I/O-free — the service feeds it oracle prices; it returns per-maker credits
//! the payout layer settles.

use std::collections::BTreeMap;

use alloy_primitives::{Address, U256};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

use crate::primitives::routing::RouteLeg;
use crate::primitives::shared::valuation::pow10;
use crate::primitives::{Bps, IntentId, MakerId, Usd, UsdPrice};

/// A token's fair-value inputs: its oracle USD price and base-unit decimals. The service builds one
/// per token a fill touched (both leg sides and the spread token).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenValue {
    pub price: UsdPrice,
    pub decimals: u8,
}

/// How a fill's recaptured LVR is shared — operator config.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct RecapturePolicy {
    /// The maker's cut of a leg's recaptured LVR.
    pub maker_share: Bps,
    /// Ceiling on a fill's total rebates, as a fraction of its realized spread.
    pub spread_cap: Bps,
    /// Dust floor: a credit worth less than this is dropped (gas would exceed it).
    pub min_credit: Usd,
}

/// One maker's rebate from a fill, denominated in `token` — the leg's input, the side the maker's
/// pool was drained of.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecaptureCredit {
    pub maker: MakerId,
    pub token: Address,
    pub amount: U256,
}

impl RecaptureCredit {
    pub fn new(maker: MakerId, token: Address, amount: U256) -> RecaptureCredit {
        RecaptureCredit {
            maker,
            token,
            amount,
        }
    }
}

/// A stored credit tagged with the intent it belongs to — the payout worker's unit of settlement.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AccruedCredit {
    pub intent: IntentId,
    pub credit: RecaptureCredit,
}

impl AccruedCredit {
    pub fn new(intent: IntentId, credit: RecaptureCredit) -> AccruedCredit {
        AccruedCredit { intent, credit }
    }
}

/// Split a confirmed fill's recaptured LVR across the makers it rebalanced.
///
/// Each leg's LVR is the USD value the maker gave (`amount_out` of `token_out`) beyond what it took
/// in (`amount_in` of `token_in`) — positive only when the maker sold below the oracle mid, i.e. a
/// rebalancing leg; imbalancing legs compute ≤ 0 and are skipped. The maker's `maker_share` of that,
/// aggregated per `(maker, token_in)` and scaled so the fill's total stays within `spread_cap` of
/// its realized `fill_spread`, is returned as a credit in `token_in`. A leg or token that cannot be
/// valued (missing or non-positive price, or an amount outside `Decimal` range) contributes nothing:
/// recapture is fail-closed on the credit, never on the fill. Direction-agnostic — exact-in and
/// exact-out differ only in which token the caller passes as `spread_token`.
pub fn recapture_split(
    legs: &[RouteLeg],
    prices: &BTreeMap<Address, TokenValue>,
    fill_spread: U256,
    spread_token: Address,
    policy: &RecapturePolicy,
) -> Vec<RecaptureCredit> {
    let cap_usd = usd_value(fill_spread, prices.get(&spread_token))
        .map_or(Decimal::ZERO, |spread| {
            spread.0 * fraction(policy.spread_cap)
        });

    let mut owed: BTreeMap<(MakerId, Address), Decimal> = BTreeMap::new();
    let mut total = Decimal::ZERO;
    for leg in legs {
        let (Some(given), Some(received)) = (
            usd_value(leg.amount_out, prices.get(&leg.token_out)),
            usd_value(leg.amount_in, prices.get(&leg.token_in)),
        ) else {
            continue;
        };
        let lvr = given.0 - received.0;
        if lvr <= Decimal::ZERO {
            continue;
        }
        let rebate = lvr * fraction(policy.maker_share);
        *owed.entry((leg.maker, leg.token_in)).or_default() += rebate;
        total += rebate;
    }

    // Scale the makers' shares down together if they would exceed the fill's spread cap.
    let scale = if total > cap_usd {
        cap_usd / total
    } else {
        Decimal::ONE
    };

    owed.into_iter()
        .filter_map(|((maker, token), rebate)| {
            let credit = Usd(rebate * scale);
            if credit < policy.min_credit {
                return None;
            }
            let amount = base_units(credit, prices.get(&token)?)?;
            (amount > U256::ZERO).then_some(RecaptureCredit {
                maker,
                token,
                amount,
            })
        })
        .collect()
}

/// A basis-point config as a plain fraction (`Bps(8000)` → `0.8`).
fn fraction(bps: Bps) -> Decimal {
    bps.0 / Decimal::from(10_000u16)
}

/// USD value of a base-unit `amount` of a token, or `None` if it is unpriced or out of `Decimal`
/// range.
fn usd_value(amount: U256, token: Option<&TokenValue>) -> Option<Usd> {
    let token = token?;
    token.price.value(whole_tokens(amount, token.decimals)?)
}

/// The floored base-unit amount of `token` worth `usd`, or `None` if unpriced or out of range.
fn base_units(usd: Usd, token: &TokenValue) -> Option<U256> {
    if token.price.0 <= Decimal::ZERO {
        return None;
    }
    (usd.0 / token.price.0)
        .checked_mul(pow10(token.decimals))
        .map(|whole| whole.floor())
        .and_then(|base| base.to_u128())
        .map(U256::from)
}

/// A base-unit `amount` as whole tokens, or `None` if it exceeds `Decimal`'s range.
fn whole_tokens(amount: U256, decimals: u8) -> Option<Decimal> {
    let base = amount.to_string().parse::<Decimal>().ok()?;
    Some(base / pow10(decimals))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::StrategyHash;
    use alloy_primitives::B256;

    fn tok(n: u8) -> Address {
        Address::from([n; 20])
    }
    fn eth() -> Address {
        tok(1)
    }
    fn usdc() -> Address {
        tok(2)
    }
    fn maker(n: u8) -> MakerId {
        MakerId(Address::from([n; 20]))
    }
    fn e18(n: u64) -> U256 {
        U256::from(n) * U256::from(10u64).pow(U256::from(18u64))
    }
    fn e6(n: u64) -> U256 {
        U256::from(n) * U256::from(10u64).pow(U256::from(6u64))
    }

    /// A leg where the maker receives `amount_in` of `token_in` and gives `amount_out` of
    /// `token_out`.
    fn leg(
        maker_byte: u8,
        strategy_byte: u8,
        token_in: Address,
        amount_in: U256,
        token_out: Address,
        amount_out: U256,
    ) -> RouteLeg {
        RouteLeg {
            maker: maker(maker_byte),
            strategy_hash: StrategyHash(B256::from([strategy_byte; 32])),
            token_in,
            token_out,
            amount_in,
            amount_out,
        }
    }

    /// ETH at $3000/18dp and USDC at $1/6dp — the design's worked example.
    fn prices() -> BTreeMap<Address, TokenValue> {
        BTreeMap::from([
            (
                eth(),
                TokenValue {
                    price: UsdPrice(Decimal::from(3000u32)),
                    decimals: 18,
                },
            ),
            (
                usdc(),
                TokenValue {
                    price: UsdPrice(Decimal::from(1u32)),
                    decimals: 6,
                },
            ),
        ])
    }

    fn policy(maker_share: u32, spread_cap: u32, min_credit: u32) -> RecapturePolicy {
        RecapturePolicy {
            maker_share: Bps(Decimal::from(maker_share)),
            spread_cap: Bps(Decimal::from(spread_cap)),
            min_credit: Usd(Decimal::from(min_credit)),
        }
    }

    // A reverse leg — maker gives 1 ETH ($3000) for 2700 USDC — recaptures 80% of the $300 LVR,
    // paid in USDC: 240 USDC.
    #[test]
    fn reverse_leg_credits_the_maker() {
        let legs = [leg(1, 1, usdc(), e6(2700), eth(), e18(1))];
        let credits = recapture_split(&legs, &prices(), e6(300), usdc(), &policy(8000, 10000, 0));
        assert_eq!(
            credits,
            vec![RecaptureCredit {
                maker: maker(1),
                token: usdc(),
                amount: e6(240),
            }]
        );
    }

    // A forward (imbalancing) leg — maker buys 1 ETH cheaply for 2700 USDC — has no LVR to return.
    #[test]
    fn forward_leg_yields_no_credit() {
        let legs = [leg(1, 1, eth(), e18(1), usdc(), e6(2700))];
        let credits = recapture_split(&legs, &prices(), e6(300), usdc(), &policy(8000, 10000, 0));
        assert!(credits.is_empty());
    }

    // The spread cap binds: a $150 (50% of $300) cap scales the $240 rebate down to $150.
    #[test]
    fn spread_cap_scales_the_total_down() {
        let legs = [leg(1, 1, usdc(), e6(2700), eth(), e18(1))];
        let credits = recapture_split(&legs, &prices(), e6(300), usdc(), &policy(8000, 5000, 0));
        assert_eq!(credits.first().map(|c| c.amount), Some(e6(150)));
    }

    // A rebate below the dust floor is dropped: $1 LVR × 80% = $0.80 < $1 min.
    #[test]
    fn dust_floor_drops_a_small_credit() {
        let legs = [leg(1, 1, usdc(), e6(2999), eth(), e18(1))];
        let credits = recapture_split(&legs, &prices(), e6(300), usdc(), &policy(8000, 10000, 1));
        assert!(credits.is_empty());
    }

    // A leg whose token is unpriced contributes nothing (fail-closed), even with a priced spread.
    #[test]
    fn missing_price_skips_the_leg() {
        let mut p = prices();
        p.remove(&eth());
        let legs = [leg(1, 1, usdc(), e6(2700), eth(), e18(1))];
        let credits = recapture_split(&legs, &p, e6(300), usdc(), &policy(8000, 10000, 0));
        assert!(credits.is_empty());
    }

    // Two rebalancing legs on the same maker/token aggregate into one credit: 2 × $240 = 480 USDC.
    #[test]
    fn same_maker_legs_aggregate() {
        let legs = [
            leg(1, 1, usdc(), e6(2700), eth(), e18(1)),
            leg(1, 2, usdc(), e6(2700), eth(), e18(1)),
        ];
        let credits = recapture_split(&legs, &prices(), e6(600), usdc(), &policy(8000, 10000, 0));
        assert_eq!(
            credits,
            vec![RecaptureCredit {
                maker: maker(1),
                token: usdc(),
                amount: e6(480),
            }]
        );
    }
}
