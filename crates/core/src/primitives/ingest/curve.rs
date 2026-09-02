//! The amount curve an intent carries on each side: a fixed amount, or a linear (Dutch) decay
//! priced at a point in time. Rounding is fixed per curve to match the source protocol's on-chain
//! decay math — the direction of the slope decides it, not the input/output role — so every reader
//! (routing, the ledger's firm confirm, tests) gets the same amount without re-deriving the rule.

use alloy_primitives::U256;

use crate::primitives::pricing::Ratio;

/// Which way a decayed amount rounds, set once from the source protocol's on-chain math.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rounding {
    Up,
    Down,
}

/// An amount over time: a fixed scalar, or a linear decay between two `(time, amount)` points.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AmountCurve {
    Static(U256),
    Linear {
        start: U256,
        end: U256,
        start_time: u64,
        end_time: u64,
        rounding: Rounding,
    },
}

impl AmountCurve {
    /// A non-decaying amount.
    pub fn scalar(amount: U256) -> AmountCurve {
        AmountCurve::Static(amount)
    }

    /// The amount at unix time `t`, clamped to the curve's `[start_time, end_time]` window.
    pub fn amount_at(&self, t: u64) -> U256 {
        match self {
            AmountCurve::Static(amount) => *amount,
            AmountCurve::Linear {
                start,
                end,
                start_time,
                end_time,
                rounding,
            } => {
                if t <= *start_time {
                    return *start;
                }
                if t >= *end_time {
                    return *end;
                }
                // start_time < t < end_time ⇒ the window is non-empty.
                let frac = Ratio::new(
                    U256::from(t - start_time),
                    U256::from(end_time - start_time),
                )
                .expect("end_time > start_time inside the open interval");
                let rising = end >= start;
                let magnitude = if rising { *end - *start } else { *start - *end };
                let moved = Ratio::from(magnitude) * frac;
                let delta = match rounding {
                    Rounding::Up => moved.ceil(),
                    Rounding::Down => moved.floor(),
                }
                .expect("delta ≤ magnitude ≤ U256::MAX");
                if rising {
                    *start + delta
                } else {
                    *start - delta
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linear(start: u64, end: u64, rounding: Rounding) -> AmountCurve {
        AmountCurve::Linear {
            start: U256::from(start),
            end: U256::from(end),
            start_time: 100,
            end_time: 200,
            rounding,
        }
    }

    #[test]
    fn static_curve_is_constant() {
        let c = AmountCurve::scalar(U256::from(42u64));
        assert_eq!(c.amount_at(0), U256::from(42u64));
        assert_eq!(c.amount_at(u64::MAX), U256::from(42u64));
    }

    #[test]
    fn linear_clamps_outside_the_window() {
        let c = linear(1000, 0, Rounding::Down);
        assert_eq!(c.amount_at(50), U256::from(1000u64)); // before start
        assert_eq!(c.amount_at(100), U256::from(1000u64)); // at start
        assert_eq!(c.amount_at(200), U256::ZERO); // at end
        assert_eq!(c.amount_at(999), U256::ZERO); // after end
    }

    #[test]
    fn decay_down_rounds_toward_floor() {
        // 1000 → 0 over [100,200]; at t=101 the drop is 1000·(1/100)=10 exactly.
        let c = linear(1000, 0, Rounding::Down);
        assert_eq!(c.amount_at(101), U256::from(990u64));
        // A fractional case: drop of 1000·(1/3) = 333.33 → floor 333 → 667 remaining.
        let c3 = AmountCurve::Linear {
            start: U256::from(1000u64),
            end: U256::ZERO,
            start_time: 0,
            end_time: 3,
            rounding: Rounding::Down,
        };
        assert_eq!(c3.amount_at(1), U256::from(667u64));
    }

    #[test]
    fn decay_up_rounds_toward_ceil() {
        // 0 → 1000 over [0,3]; at t=1 the rise is 1000·(1/3)=333.33 → ceil 334.
        let c = AmountCurve::Linear {
            start: U256::ZERO,
            end: U256::from(1000u64),
            start_time: 0,
            end_time: 3,
            rounding: Rounding::Up,
        };
        assert_eq!(c.amount_at(1), U256::from(334u64));
    }

    #[test]
    fn degenerate_window_does_not_divide_by_zero() {
        let c = AmountCurve::Linear {
            start: U256::from(5u64),
            end: U256::from(9u64),
            start_time: 100,
            end_time: 100,
            rounding: Rounding::Up,
        };
        assert_eq!(c.amount_at(99), U256::from(5u64)); // t ≤ start_time
        assert_eq!(c.amount_at(100), U256::from(5u64)); // t == both
        assert_eq!(c.amount_at(101), U256::from(9u64)); // t ≥ end_time
    }
}
