//! Executor-fee policy shared by quote construction and signed-order validation.

use alloy_primitives::U256;

use crate::SolventError;

const BPS_DENOMINATOR: u64 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionFeePolicy {
    bps: u32,
}

impl ExecutionFeePolicy {
    pub fn new(bps: u32) -> Result<Self, SolventError> {
        if bps == 0 || bps >= BPS_DENOMINATOR as u32 {
            return Err(SolventError::InvalidId {
                id_type: "executor fee bps",
                reason: "must be between 1 and 9,999".to_string(),
            });
        }
        Ok(Self { bps })
    }

    pub fn bps(self) -> u32 {
        self.bps
    }

    /// Splitting before multiplication keeps the calculation exact without overflowing U256.
    pub fn fee(self, gross_input: U256) -> Result<U256, SolventError> {
        let denominator = U256::from(BPS_DENOMINATOR);
        let bps = U256::from(self.bps);
        let quotient = gross_input / denominator;
        let remainder = gross_input % denominator;
        let base = quotient.checked_mul(bps).ok_or_else(fee_overflow)?;
        let rounded_remainder = remainder
            .checked_mul(bps)
            .and_then(|value| value.checked_add(denominator - U256::from(1u8)))
            .map(|value| value / denominator)
            .ok_or_else(fee_overflow)?;
        let fee = base
            .checked_add(rounded_remainder)
            .ok_or_else(fee_overflow)?;
        Ok(fee.max(U256::from(1u8)))
    }

    pub fn routing_input(self, gross_input: U256) -> Result<U256, SolventError> {
        let fee = self.fee(gross_input)?;
        gross_input
            .checked_sub(fee)
            .filter(|amount| !amount.is_zero())
            .ok_or_else(|| SolventError::InvalidId {
                id_type: "amount",
                reason: "the input amount must exceed the executor fee".to_string(),
            })
    }
}

fn fee_overflow() -> SolventError {
    SolventError::InvalidId {
        id_type: "amount",
        reason: "executor fee calculation overflowed".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fee_rounds_up_and_never_returns_zero() {
        let policy = ExecutionFeePolicy::new(5).expect("valid policy");
        assert_eq!(policy.fee(U256::from(1u8)).expect("fee"), U256::from(1u8));
        assert_eq!(
            policy.fee(U256::from(10_001u64)).expect("fee"),
            U256::from(6u8)
        );
    }

    #[test]
    fn fee_handles_the_full_uint256_domain() {
        let policy = ExecutionFeePolicy::new(9_999).expect("valid policy");
        let fee = policy.fee(U256::MAX).expect("fee");
        assert!(fee < U256::MAX);
        assert_eq!(
            policy.routing_input(U256::MAX).expect("net") + fee,
            U256::MAX
        );
    }
}
