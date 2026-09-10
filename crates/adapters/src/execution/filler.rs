//! Shared wire schema for the policy-authorized filler contract.

use alloy::sol;

use solvent_core::primitives::execution::ExecutionAuthorization;

sol! {
    struct Order {
        address maker;
        uint256 traits;
        bytes data;
    }
    struct SignedOrder {
        bytes order;
        bytes sig;
    }
    struct Authorization {
        uint8 kind;
        uint256 nonce;
        bytes32 contextHash;
        bytes32 strategyHash;
        address maker;
        address tokenIn;
        address tokenOut;
        uint256 amountOut;
        uint256 amountInLimit;
        uint256 rebateAmount;
        uint64 deadlineBlock;
    }
    struct SourceSwap {
        Order order;
        Authorization authorization;
        bytes policySignature;
    }
    function fill(SignedOrder order, SourceSwap[] sources);
}

impl From<&ExecutionAuthorization> for Authorization {
    fn from(value: &ExecutionAuthorization) -> Self {
        Self {
            kind: value.kind.as_u8(),
            nonce: value.nonce,
            contextHash: value.context_hash,
            strategyHash: value.strategy_hash.0,
            maker: value.maker.0,
            tokenIn: value.token_in,
            tokenOut: value.token_out,
            amountOut: value.amount_out,
            amountInLimit: value.amount_in_limit,
            rebateAmount: value.rebate_amount,
            deadlineBlock: value.deadline_block,
        }
    }
}
