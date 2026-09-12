//! Alloy-generated ABI and EIP-712 types matching `SolventSameChainSettler`.

use alloy::primitives::{Address, B256};
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolStruct};

sol! {
    struct Solvent7683Order {
        address settler;
        address user;
        uint256 chainId;
        address inputToken;
        uint256 inputAmount;
        address outputToken;
        uint256 outputAmount;
        address recipient;
        uint256 executorFee;
        uint256 nonce;
        uint256 deadline;
    }

    struct TokenPermissions {
        address token;
        uint256 amount;
    }

    struct PermitWitnessTransferFrom {
        TokenPermissions permitted;
        address spender;
        uint256 nonce;
        uint256 deadline;
        Solvent7683Order witness;
    }

    function fill(
        bytes orderData,
        bytes permitSignature,
        bytes sourceData,
        address paymentRecipient
    ) returns (uint256 earned);
}

impl Solvent7683Order {
    pub fn order_id(&self) -> B256 {
        self.eip712_hash_struct()
    }

    pub fn permit_digest(&self, permit2: Address, chain_id: u64) -> B256 {
        let permit = PermitWitnessTransferFrom {
            permitted: TokenPermissions {
                token: self.inputToken,
                amount: self.inputAmount,
            },
            spender: self.settler,
            nonce: self.nonce,
            deadline: self.deadline,
            witness: self.clone(),
        };
        permit.eip712_signing_hash(&eip712_domain! {
            name: "Permit2",
            chain_id: chain_id,
            verifying_contract: permit2,
        })
    }
}

pub(crate) fn order_id(order: &Solvent7683Order) -> B256 {
    order.order_id()
}

pub(crate) fn permit_digest(order: &Solvent7683Order, permit2: Address, chain_id: u64) -> B256 {
    order.permit_digest(permit2, chain_id)
}
