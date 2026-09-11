//! Pure policy for deciding whether a displaced strategy remains routable or funds a rebate.

use alloy_primitives::{Address, U256};
use thiserror::Error;

use crate::ledger::AvailableSnapshot;
use crate::primitives::pricing::Ratio;
use crate::primitives::rebate::{
    RebateAccrual, RebateAllocation, RebateAssessment, RebateMarket, RebatePlan, RebateRequirements,
};
use crate::primitives::registry::{MakerStrategy, TokenPair};
use crate::primitives::RebateBatchId;
use crate::registry::CurveError;
use crate::routing::candidates::build_pricing_candidate;
use crate::routing::Candidate;

const BASIS_POINTS: u64 = 10_000;
const EXECUTOR_SHARE_BPS: u64 = 500;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum RebateError {
    #[error("rebate accrual is invalid: {0}")]
    InvalidAccrual(&'static str),
    #[error("rebate input does not match strategy {0}")]
    StrategyMismatch(crate::primitives::StrategyHash),
    #[error("rebate execution is invalid: {0}")]
    InvalidExecution(&'static str),
    #[error("market pair does not match the strategy")]
    MarketPairMismatch,
    #[error("rebate economics are missing for input token {0}")]
    MissingRequirements(Address),
    #[error("rebate arithmetic exceeded its domain")]
    Arithmetic,
    #[error("rebate curve: {0}")]
    Curve(#[from] CurveError),
}

/// Pair-wide deviation and gas-safety policy; token-denominated minima arrive with each quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebatePolicyConfig {
    deviation_threshold_bps: u64,
    /// Multiplier over estimated gas (`10_000` = 1.0x), rounded up.
    gas_safety_bps: u64,
}

impl RebatePolicyConfig {
    pub fn new(deviation_threshold_bps: u64, gas_safety_bps: u64) -> Self {
        Self {
            deviation_threshold_bps,
            gas_safety_bps: gas_safety_bps.max(BASIS_POINTS),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RebatePolicy {
    config: RebatePolicyConfig,
}

impl RebatePolicy {
    pub fn new(config: RebatePolicyConfig) -> Self {
        Self { config }
    }

    pub(crate) fn assess(
        &self,
        batch_id: RebateBatchId,
        accruals: &[RebateAccrual],
        strategy: &MakerStrategy,
        caps: &AvailableSnapshot,
        market: &RebateMarket,
        requirements: &[RebateRequirements],
    ) -> Result<RebateAssessment, RebateError> {
        if accruals
            .iter()
            .any(|accrual| accrual.strategy != strategy.key)
        {
            return Err(RebateError::StrategyMismatch(strategy.key.strategy_hash));
        }
        let Some(pair) = strategy.pair() else {
            return Err(RebateError::MarketPairMismatch);
        };
        if accruals
            .iter()
            .any(|accrual| TokenPair::new(accrual.token_in, accrual.token_out) != pair)
        {
            return Err(RebateError::InvalidAccrual(
                "trade pair does not match the strategy",
            ));
        }
        if pair != TokenPair::new(market.token_a, market.token_b)
            || market.token_a == market.token_b
            || market.a_to_b.is_zero()
            || market.b_to_a.is_zero()
        {
            return Err(RebateError::MarketPairMismatch);
        }

        let best = self.best_direction(strategy, caps, market)?;
        let Some(opportunity) = best else {
            return Ok(RebateAssessment::Quoteable { deviation_bps: 0 });
        };
        // Small displacements keep quoting so several trades can share one profitable restoration.
        if opportunity.deviation_bps < self.config.deviation_threshold_bps {
            return Ok(RebateAssessment::Quoteable {
                deviation_bps: opportunity.deviation_bps,
            });
        }
        if opportunity.candidate.cap_out.is_zero() {
            return Ok(RebateAssessment::Guarded {
                deviation_bps: opportunity.deviation_bps,
            });
        }

        // Reuse the routed curve's price-limit primitive so rebate and swap pricing cannot diverge.
        let fill = opportunity.candidate.net_quote_with_limit(
            opportunity.candidate.full_input_bound(),
            &opportunity.price_limit,
        )?;
        if fill.amount_in.is_zero() || fill.amount_out.is_zero() {
            return Ok(RebateAssessment::Guarded {
                deviation_bps: opportunity.deviation_bps,
            });
        }
        if !fill.limited {
            return Ok(RebateAssessment::Guarded {
                deviation_bps: opportunity.deviation_bps,
            });
        }
        let amount_in = opportunity.candidate.net_quote_exact_out(fill.amount_out)?;
        let external_value = (Ratio::from(fill.amount_out) * opportunity.output_value_rate)
            .floor()
            .ok_or(RebateError::Arithmetic)?;
        let Some(gross_surplus) = external_value.checked_sub(amount_in) else {
            return Ok(RebateAssessment::Guarded {
                deviation_bps: opportunity.deviation_bps,
            });
        };
        // Gas is paid first; only economically realizable surplus is shared with the maker.
        let requirements = requirements
            .iter()
            .find(|requirements| requirements.token == opportunity.candidate.token_in)
            .ok_or(RebateError::MissingRequirements(
                opportunity.candidate.token_in,
            ))?;
        let safe_gas_cost = scale_bps(requirements.gas_cost, self.config.gas_safety_bps, true)?;
        let Some(post_gas_surplus) = gross_surplus.checked_sub(safe_gas_cost) else {
            return Ok(RebateAssessment::Guarded {
                deviation_bps: opportunity.deviation_bps,
            });
        };
        let executor_profit = scale_bps(post_gas_surplus, EXECUTOR_SHARE_BPS, false)?;
        let maker_rebate = post_gas_surplus
            .checked_sub(executor_profit)
            .ok_or(RebateError::Arithmetic)?;
        if maker_rebate < requirements.minimum_maker_rebate
            || executor_profit < requirements.minimum_executor_profit
        {
            return Ok(RebateAssessment::Guarded {
                deviation_bps: opportunity.deviation_bps,
            });
        }

        Ok(RebateAssessment::Ready(Box::new(RebatePlan {
            batch_id,
            strategy: strategy.key,
            token_in: opportunity.candidate.token_in,
            token_out: opportunity.candidate.token_out,
            amount_in,
            amount_out: fill.amount_out,
            gross_surplus,
            safe_gas_cost,
            maker_rebate,
            executor_profit,
            deviation_bps: opportunity.deviation_bps,
            allocations: allocate(maker_rebate, accruals)?,
        })))
    }

    fn best_direction(
        &self,
        strategy: &MakerStrategy,
        caps: &AvailableSnapshot,
        market: &RebateMarket,
    ) -> Result<Option<Direction>, RebateError> {
        let directions = [
            (market.token_a, market.token_b),
            (market.token_b, market.token_a),
        ];
        let mut best: Option<Direction> = None;
        for (token_in, token_out) in directions {
            // For A -> B, the executable B -> A side is the adverse value the executor can realize
            // for B. Its inverse is the highest A -> B curve price that still restores profitably.
            let output_value_rate = market
                .rate(token_out, token_in)
                .cloned()
                .ok_or(RebateError::MarketPairMismatch)?;
            let price_limit = output_value_rate
                .clone()
                .invert()
                .ok_or(RebateError::MarketPairMismatch)?;
            let Some(candidate) = build_pricing_candidate(strategy, caps, token_in, token_out)
            else {
                continue;
            };
            let marginal = candidate.marginal_price()?;
            if marginal <= price_limit {
                continue;
            }
            let direction = Direction {
                deviation_bps: marginal.rel_diff_bps(&price_limit),
                candidate,
                price_limit,
                output_value_rate,
            };
            if best
                .as_ref()
                .is_none_or(|current| direction.deviation_bps > current.deviation_bps)
            {
                best = Some(direction);
            }
        }
        Ok(best)
    }
}

struct Direction {
    candidate: Candidate,
    price_limit: Ratio,
    output_value_rate: Ratio,
    deviation_bps: u64,
}

fn scale_bps(amount: U256, bps: u64, round_up: bool) -> Result<U256, RebateError> {
    let ratio =
        Ratio::new(U256::from(bps), U256::from(BASIS_POINTS)).ok_or(RebateError::Arithmetic)?;
    let scaled = Ratio::from(amount) * ratio;
    match round_up {
        true => scaled.ceil(),
        false => scaled.floor(),
    }
    .ok_or(RebateError::Arithmetic)
}

fn allocate(
    maker_rebate: U256,
    accruals: &[RebateAccrual],
) -> Result<Vec<RebateAllocation>, RebateError> {
    let total_weight = accruals.iter().try_fold(U256::ZERO, |total, accrual| {
        total
            .checked_add(accrual.allocation_weight)
            .ok_or(RebateError::Arithmetic)
    })?;
    if total_weight.is_zero() {
        return Err(RebateError::InvalidAccrual("allocation weight is zero"));
    }

    let mut allocated = U256::ZERO;
    let last = accruals.len().saturating_sub(1);
    accruals
        .iter()
        .enumerate()
        .map(|(index, accrual)| {
            let amount = if index == last {
                // Assign integer-division dust to the maker through the final affected trade.
                maker_rebate
                    .checked_sub(allocated)
                    .ok_or(RebateError::Arithmetic)?
            } else {
                (Ratio::from(maker_rebate)
                    * Ratio::new(accrual.allocation_weight, total_weight)
                        .ok_or(RebateError::Arithmetic)?)
                .floor()
                .ok_or(RebateError::Arithmetic)?
            };
            allocated = allocated
                .checked_add(amount)
                .ok_or(RebateError::Arithmetic)?;
            Ok(RebateAllocation {
                trade_id: accrual.trade_id,
                amount,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use alloy_primitives::{Address, B256};
    use ulid::Ulid;

    use crate::primitives::ledger::AccountKey;
    use crate::primitives::rebate::RebateAccrual;
    use crate::primitives::registry::{Curve, CurveSpec, PeggedParams, StrategyKey};
    use crate::primitives::trade::TradeId;
    use crate::primitives::{MakerId, StrategyHash};

    fn token(n: u8) -> Address {
        Address::from([n; 20])
    }

    fn units(n: u64) -> U256 {
        U256::from(n) * U256::from(10u64).pow(U256::from(18u64))
    }

    fn key(n: u8) -> StrategyKey {
        StrategyKey {
            maker: MakerId(token(n)),
            app: Address::ZERO,
            strategy_hash: StrategyHash(B256::from([n; 32])),
        }
    }

    fn strategy(curve: Curve) -> MakerStrategy {
        let reserve = units(1_000);
        MakerStrategy {
            key: key(1),
            curve: CurveSpec::Priceable {
                curve,
                fees_in_bps: vec![],
            },
            balances: BTreeMap::from([(token(1), reserve), (token(2), reserve)]),
            active: true,
            program: Default::default(),
        }
    }

    fn curves() -> [Curve; 3] {
        let reserve = units(1_000);
        [
            Curve::Xyc,
            Curve::Concentrate {
                sqrt_price_min: U256::from(500_000_000_000_000_000u64),
                sqrt_price_max: U256::from(2_000_000_000_000_000_000u64),
            },
            Curve::Pegged(PeggedParams {
                x0: reserve,
                y0: reserve,
                linear_width: U256::from(100u64) * U256::from(10u64).pow(U256::from(27u64)),
                rate_lt: U256::from(1u64),
                rate_gt: U256::from(1u64),
            }),
        ]
    }

    fn caps(strategy: &MakerStrategy) -> AvailableSnapshot {
        let amount = units(1_000);
        AvailableSnapshot(BTreeMap::from([
            (
                AccountKey::WalletBudget {
                    maker: strategy.key.maker,
                    token: token(1),
                },
                amount,
            ),
            (
                AccountKey::WalletBudget {
                    maker: strategy.key.maker,
                    token: token(2),
                },
                amount,
            ),
            (
                AccountKey::StrategyVirtual {
                    maker: strategy.key.maker,
                    strategy_hash: strategy.key.strategy_hash,
                    token: token(1),
                },
                amount,
            ),
            (
                AccountKey::StrategyVirtual {
                    maker: strategy.key.maker,
                    strategy_hash: strategy.key.strategy_hash,
                    token: token(2),
                },
                amount,
            ),
        ]))
    }

    fn accruals(strategy: &MakerStrategy) -> Vec<RebateAccrual> {
        [1u128, 2]
            .into_iter()
            .map(|n| {
                RebateAccrual::new(
                    TradeId(Ulid::from_parts(1, n)),
                    strategy.key,
                    token(1),
                    token(2),
                    units(10),
                    units(10),
                    U256::from(n),
                    1,
                )
            })
            .collect()
    }

    fn config(threshold: u64) -> RebatePolicyConfig {
        RebatePolicyConfig::new(threshold, 12_000)
    }

    fn assess(
        curve: Curve,
        config: RebatePolicyConfig,
        market: Ratio,
        gas: U256,
        minimum_executor_profit: U256,
    ) -> RebateAssessment {
        assess_oriented(
            curve,
            config,
            token(1),
            token(2),
            market,
            gas,
            minimum_executor_profit,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn assess_oriented(
        curve: Curve,
        config: RebatePolicyConfig,
        token_in: Address,
        token_out: Address,
        market: Ratio,
        gas: U256,
        minimum_executor_profit: U256,
    ) -> RebateAssessment {
        let strategy = strategy(curve);
        RebatePolicy::new(config)
            .assess(
                RebateBatchId(B256::ZERO),
                &accruals(&strategy),
                &strategy,
                &caps(&strategy),
                &RebateMarket::new(
                    token_in,
                    token_out,
                    market.clone(),
                    market.invert().unwrap(),
                ),
                &[RebateRequirements::new(
                    token_in,
                    gas,
                    U256::from(1u64),
                    minimum_executor_profit,
                )],
            )
            .unwrap()
    }

    #[test]
    fn policy_boundaries_and_split_hold_for_every_curve() {
        for curve in curves() {
            assert!(matches!(
                assess(
                    curve,
                    config(11),
                    Ratio::new(U256::from(999u64), U256::from(1_000u64)).unwrap(),
                    U256::ZERO,
                    U256::from(1u64),
                ),
                RebateAssessment::Quoteable { deviation_bps: 10 }
            ));

            let ready = assess(
                curve,
                config(100),
                Ratio::new(U256::from(9u64), U256::from(10u64)).unwrap(),
                U256::ZERO,
                U256::from(1u64),
            );
            let RebateAssessment::Ready(plan) = ready else {
                panic!("displaced curve should clear the rebate gates");
            };
            assert_eq!(plan.executor_profit, plan.gross_surplus / U256::from(20u64));
            assert_eq!(plan.maker_rebate + plan.executor_profit, plan.gross_surplus);
            assert_eq!(
                plan.allocations
                    .iter()
                    .fold(U256::ZERO, |sum, allocation| sum + allocation.amount),
                plan.maker_rebate
            );

            let reverse = assess_oriented(
                curve,
                config(100),
                token(2),
                token(1),
                Ratio::new(U256::from(9u64), U256::from(10u64)).unwrap(),
                U256::ZERO,
                U256::from(1u64),
            );
            let RebateAssessment::Ready(reverse) = reverse else {
                panic!("reverse orientation should clear the rebate gates");
            };
            assert_eq!(reverse.token_in, token(2));
            assert_eq!(reverse.token_out, token(1));

            let too_expensive = RebatePolicyConfig::new(100, BASIS_POINTS);
            assert!(matches!(
                assess(
                    curve,
                    too_expensive,
                    Ratio::new(U256::from(9u64), U256::from(10u64)).unwrap(),
                    plan.gross_surplus,
                    U256::from(1u64),
                ),
                RebateAssessment::Guarded { .. }
            ));

            assert!(matches!(
                assess(
                    curve,
                    config(100),
                    Ratio::new(U256::from(9u64), U256::from(10u64)).unwrap(),
                    U256::ZERO,
                    plan.executor_profit + U256::from(1u64),
                ),
                RebateAssessment::Guarded { .. }
            ));
        }

        let strategy = strategy(Curve::Xyc);
        let no_capacity = RebatePolicy::new(config(100))
            .assess(
                RebateBatchId(B256::ZERO),
                &accruals(&strategy),
                &strategy,
                &AvailableSnapshot::default(),
                &RebateMarket::new(
                    token(1),
                    token(2),
                    Ratio::new(U256::from(9u64), U256::from(10u64)).unwrap(),
                    Ratio::new(U256::from(10u64), U256::from(9u64)).unwrap(),
                ),
                &[RebateRequirements::new(
                    token(1),
                    U256::ZERO,
                    U256::from(1u64),
                    U256::from(1u64),
                )],
            )
            .unwrap();
        assert!(matches!(no_capacity, RebateAssessment::Guarded { .. }));
    }

    #[test]
    fn spread_values_both_orientations_at_the_executable_reverse_side() {
        let market = RebateMarket::new(
            token(1),
            token(2),
            Ratio::new(U256::from(4u64), U256::from(5u64)).unwrap(),
            Ratio::new(U256::from(11u64), U256::from(10u64)).unwrap(),
        );
        let requirements = [
            RebateRequirements::new(token(1), U256::ZERO, U256::ZERO, U256::ZERO),
            RebateRequirements::new(token(2), U256::ZERO, U256::ZERO, U256::ZERO),
        ];

        let mut forward = strategy(Curve::Xyc);
        forward.balances.insert(token(1), units(500));
        forward.balances.insert(token(2), units(1_000));
        let forward_assessment = RebatePolicy::new(config(1))
            .assess(
                RebateBatchId(B256::ZERO),
                &accruals(&forward),
                &forward,
                &caps(&forward),
                &market,
                &requirements,
            )
            .unwrap();
        let RebateAssessment::Ready(forward_plan) = forward_assessment else {
            panic!("forward orientation should clear the rebate gates");
        };
        assert_eq!(forward_plan.token_in, token(1));
        assert_eq!(forward_plan.token_out, token(2));
        let forward_value = (Ratio::from(forward_plan.amount_out) * market.b_to_a.clone())
            .floor()
            .unwrap();
        let optimistic_forward = (Ratio::from(forward_plan.amount_out)
            * market.a_to_b.clone().invert().unwrap())
        .floor()
        .unwrap();
        assert_eq!(
            forward_plan.gross_surplus,
            forward_value - forward_plan.amount_in
        );
        assert!(forward_value < optimistic_forward);

        let mut reverse = strategy(Curve::Xyc);
        reverse.balances.insert(token(1), units(1_000));
        reverse.balances.insert(token(2), units(500));
        let reverse_assessment = RebatePolicy::new(config(1))
            .assess(
                RebateBatchId(B256::ZERO),
                &accruals(&reverse),
                &reverse,
                &caps(&reverse),
                &market,
                &requirements,
            )
            .unwrap();
        let RebateAssessment::Ready(reverse_plan) = reverse_assessment else {
            panic!("reverse orientation should clear the rebate gates");
        };
        assert_eq!(reverse_plan.token_in, token(2));
        assert_eq!(reverse_plan.token_out, token(1));
        let reverse_value = (Ratio::from(reverse_plan.amount_out) * market.a_to_b.clone())
            .floor()
            .unwrap();
        let optimistic_reverse = (Ratio::from(reverse_plan.amount_out)
            * market.b_to_a.clone().invert().unwrap())
        .floor()
        .unwrap();
        assert_eq!(
            reverse_plan.gross_surplus,
            reverse_value - reverse_plan.amount_in
        );
        assert!(reverse_value < optimistic_reverse);
    }

    #[test]
    fn capacity_limited_restoration_stays_guarded() {
        let mut strategy = strategy(Curve::Xyc);
        strategy.balances.insert(token(1), units(500));
        strategy.balances.insert(token(2), units(1_000));
        let mut available = caps(&strategy);
        for account in [
            AccountKey::WalletBudget {
                maker: strategy.key.maker,
                token: token(2),
            },
            AccountKey::StrategyVirtual {
                maker: strategy.key.maker,
                strategy_hash: strategy.key.strategy_hash,
                token: token(2),
            },
        ] {
            available.0.insert(account, units(1));
        }

        let assessment = RebatePolicy::new(config(1))
            .assess(
                RebateBatchId(B256::ZERO),
                &accruals(&strategy),
                &strategy,
                &available,
                &RebateMarket::new(
                    token(1),
                    token(2),
                    Ratio::new(U256::from(4u64), U256::from(5u64)).unwrap(),
                    Ratio::new(U256::from(11u64), U256::from(10u64)).unwrap(),
                ),
                &[
                    RebateRequirements::new(token(1), U256::ZERO, U256::ZERO, U256::ZERO),
                    RebateRequirements::new(token(2), U256::ZERO, U256::ZERO, U256::ZERO),
                ],
            )
            .unwrap();

        assert!(matches!(assessment, RebateAssessment::Guarded { .. }));
    }
}
