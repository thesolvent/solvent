//! Policy authorization for one exact-output execution against a protected maker strategy.

use core::fmt;

use alloy_primitives::{Address, Bytes, B256, U256};

use crate::primitives::{MakerId, StrategyHash};

/// Identifies the execution path in the signed policy domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum ExecutionKind {
    UserFill = 0,
    Rebate = 1,
}

impl ExecutionKind {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

/// The exact execution policy signed by the resolver and validated by the filler contract.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExecutionAuthorization {
    pub kind: ExecutionKind,
    pub nonce: U256,
    pub context_hash: B256,
    pub strategy_hash: StrategyHash,
    pub maker: MakerId,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_out: U256,
    pub amount_in_limit: U256,
    pub rebate_amount: U256,
    pub deadline_block: u64,
}

impl ExecutionAuthorization {
    pub fn user_fill(value: UserFillAuthorization) -> Self {
        Self {
            kind: ExecutionKind::UserFill,
            nonce: value.nonce,
            context_hash: value.context_hash,
            strategy_hash: value.strategy_hash,
            maker: value.maker,
            token_in: value.token_in,
            token_out: value.token_out,
            amount_out: value.amount_out,
            amount_in_limit: value.amount_in_limit,
            rebate_amount: U256::ZERO,
            // The signed UniswapX order carries the user-fill expiry. Keeping this authorization
            // stable across retries preserves idempotency without opening another execution path.
            deadline_block: u64::MAX,
        }
    }

    pub fn rebate(value: RebateAuthorization) -> Self {
        Self {
            kind: ExecutionKind::Rebate,
            nonce: value.nonce,
            context_hash: value.context_hash,
            strategy_hash: value.strategy_hash,
            maker: value.maker,
            token_in: value.token_in,
            token_out: value.token_out,
            amount_out: value.amount_out,
            amount_in_limit: value.amount_in_limit,
            rebate_amount: value.rebate_amount,
            deadline_block: value.deadline_block,
        }
    }
}

/// Inputs that vary for a normal UniswapX source authorization.
pub struct UserFillAuthorization {
    pub nonce: U256,
    pub context_hash: B256,
    pub strategy_hash: StrategyHash,
    pub maker: MakerId,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_out: U256,
    pub amount_in_limit: U256,
}

/// Inputs that bind a public restoration to one batch and protected strategy.
pub struct RebateAuthorization {
    pub nonce: U256,
    pub context_hash: B256,
    pub strategy_hash: StrategyHash,
    pub maker: MakerId,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_out: U256,
    pub amount_in_limit: U256,
    pub rebate_amount: U256,
    pub deadline_block: u64,
}

/// A signature is executable authority. Its debug representation must never expose signed bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct PolicySignature(Bytes);

impl PolicySignature {
    pub fn new(bytes: Bytes) -> Self {
        Self(bytes)
    }

    pub fn into_bytes(self) -> Bytes {
        self.0
    }

    pub fn as_bytes(&self) -> &Bytes {
        &self.0
    }
}

impl fmt::Debug for PolicySignature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PolicySignature([REDACTED])")
    }
}
