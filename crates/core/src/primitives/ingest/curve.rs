//! The amount curve an intent carries on each side: a fixed amount, or a linear (Dutch) decay
//! priced at a point in time. The decay reproduces `DutchDecayLib.linearDecay` exactly: the
//! *travelled distance* floors (`mulDivDown` on `|start − end|`), and that floored delta is then
//! added to or subtracted from `start`. Flooring the distance — rather than the resulting amount —
//! is what makes both slope directions land in the swapper's favour, and interpolating the endpoint
//! instead differs by one wei on a rising curve, which the reactor rejects.

use alloy_primitives::U256;

use crate::primitives::pricing::Ratio;

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
    },
}

impl AmountCurve {
    /// A non-decaying amount.
    pub fn scalar(amount: U256) -> AmountCurve {
        AmountCurve::Static(amount)
    }

    /// A Dutch-auction curve from `start` to `end` over `[start_time, end_time]`. Collapses to
    /// `Static` when it doesn't move, mirroring the contract's `startAmount == endAmount` early
    /// return (which also bypasses its `EndTimeBeforeStartTime` guard).
    pub fn dutch(start: U256, end: U256, start_time: u64, end_time: u64) -> AmountCurve {
        if start == end {
            return AmountCurve::Static(start);
        }
        AmountCurve::Linear {
            start,
            end,
            start_time,
            end_time,
        }
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
                let delta = (Ratio::from(magnitude) * frac)
                    .floor()
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

    fn linear(start: u64, end: u64) -> AmountCurve {
        AmountCurve::Linear {
            start: U256::from(start),
            end: U256::from(end),
            start_time: 100,
            end_time: 200,
        }
    }

    fn window(start: u64, end: u64, start_time: u64, end_time: u64) -> AmountCurve {
        AmountCurve::Linear {
            start: U256::from(start),
            end: U256::from(end),
            start_time,
            end_time,
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
        let c = linear(1000, 0);
        assert_eq!(c.amount_at(50), U256::from(1000u64)); // before start
        assert_eq!(c.amount_at(100), U256::from(1000u64)); // at start
        assert_eq!(c.amount_at(200), U256::ZERO); // at end
        assert_eq!(c.amount_at(999), U256::ZERO); // after end
    }

    /// A falling curve: the floored distance is subtracted, so the amount rounds up.
    #[test]
    fn falling_curve_floors_the_distance() {
        // 1000 → 0 over [100,200]; at t=101 the drop is 1000·(1/100) = 10 exactly.
        assert_eq!(linear(1000, 0).amount_at(101), U256::from(990u64));
        // Fractional: 1000·(1/3) = 333.33 → floor 333 → 667 remaining.
        assert_eq!(window(1000, 0, 0, 3).amount_at(1), U256::from(667u64));
    }

    /// A rising curve floors the same distance — `DutchDecayLib` uses `mulDivDown` on `|Δ|` in both
    /// branches, so the amount rounds *down* here. Ceiling it (interpolating the endpoint) yields
    /// 334 and is one wei over what the reactor computes.
    #[test]
    fn rising_curve_floors_the_distance() {
        assert_eq!(window(0, 1000, 0, 3).amount_at(1), U256::from(333u64));
    }

    #[test]
    fn dutch_collapses_only_a_flat_curve() {
        assert_eq!(
            AmountCurve::dutch(U256::from(10u64), U256::from(4u64), 0, 5),
            window(10, 4, 0, 5)
        );
        assert_eq!(
            AmountCurve::dutch(U256::from(4u64), U256::from(10u64), 0, 5),
            window(4, 10, 0, 5)
        );
        assert_eq!(
            AmountCurve::dutch(U256::from(7u64), U256::from(7u64), 0, 5),
            AmountCurve::Static(U256::from(7u64))
        );
    }

    #[test]
    fn degenerate_window_does_not_divide_by_zero() {
        let c = window(5, 9, 100, 100);
        assert_eq!(c.amount_at(99), U256::from(5u64)); // t ≤ start_time
        assert_eq!(c.amount_at(100), U256::from(5u64)); // t == both
        assert_eq!(c.amount_at(101), U256::from(9u64)); // t ≥ end_time
    }

    /// `DutchDecayLib.decay` transcribed independently in `u128`, as the differential oracle. The
    /// on-chain differential lives in the fork E2E; this catches the arithmetic drift cheaply.
    fn reference(start: u128, end: u128, start_time: u64, end_time: u64, t: u64) -> u128 {
        if start == end {
            return start;
        }
        if end_time <= t {
            return end;
        }
        if start_time >= t {
            return start;
        }
        let elapsed = u128::from(t - start_time);
        let duration = u128::from(end_time - start_time);
        match end < start {
            true => start - (start - end) * elapsed / duration,
            false => start + (end - start) * elapsed / duration,
        }
    }

    #[test]
    fn matches_the_contract_across_both_slopes() {
        // Amounts and windows chosen so the division is fractional far more often than not.
        let cases = [
            (1_000_000_000u128, 0u128),
            (0, 1_000_000_000),
            (3, 1),
            (1, 3),
            (997, 13),
            (13, 997),
            (u128::from(u64::MAX), u128::from(u64::MAX) - 7),
        ];
        let windows = [(100u64, 103u64), (100, 160), (0, 7), (1_000, 1_997)];
        let mut checked = 0;
        for (start, end) in cases {
            for (start_time, end_time) in windows {
                let curve =
                    AmountCurve::dutch(U256::from(start), U256::from(end), start_time, end_time);
                for t in start_time.saturating_sub(2)..=end_time + 2 {
                    let expected = reference(start, end, start_time, end_time, t);
                    assert_eq!(
                        curve.amount_at(t),
                        U256::from(expected),
                        "start={start} end={end} window=[{start_time},{end_time}] t={t}"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked >= 100, "expected a broad sweep, checked {checked}");
    }
}
