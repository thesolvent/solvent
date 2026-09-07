//! ERC-7683 v1 wire types and the order id. The envelope is the standard's
//! `GaslessCrossChainOrder`; `orderData` carries the `SolventOrder` order type, selected by
//! `orderDataType`. Both are plain EIP-712 structs, so alloy derives the type strings and the
//! struct hash — unlike UniswapX, nothing here is hand-composed.

use alloy::primitives::{keccak256, B256};
use alloy::sol;
use alloy::sol_types::SolStruct;

sol! {
    struct GaslessCrossChainOrder {
        address originSettler;
        address user;
        uint256 nonce;
        uint256 originChainId;
        uint32 openDeadline;
        uint32 fillDeadline;
        bytes32 orderDataType;
        bytes orderData;
    }

    struct SolventOrder {
        address inputToken;
        uint256 inputAmount;
        address outputToken;
        uint256 outputAmount;
        address recipient;
        address exclusiveFiller;
        uint32 exclusivityEnds;
    }
}

/// The `orderDataType` selecting our order type; the settler accepts no other.
pub(crate) fn solvent_order_type() -> B256 {
    keccak256(SolventOrder::eip712_encode_type().as_bytes())
}

/// The order id the settler records: the envelope's EIP-712 struct hash.
pub(crate) fn order_id(order: &GaslessCrossChainOrder) -> B256 {
    order.eip712_hash_struct()
}

#[cfg(test)]
mod tests {
    use super::*;

    // These two strings are the cross-language contract with `SameChainSettler`. Alloy derives them
    // from the `sol!` declarations, so a reordered or renamed field would silently change the type
    // hash and diverge from the contract; pinning them here makes that a test failure.
    #[test]
    fn eip712_type_strings_are_pinned() {
        assert_eq!(
            SolventOrder::eip712_encode_type(),
            "SolventOrder(address inputToken,uint256 inputAmount,address outputToken,\
uint256 outputAmount,address recipient,address exclusiveFiller,uint32 exclusivityEnds)"
        );
        assert_eq!(
            GaslessCrossChainOrder::eip712_encode_type(),
            "GaslessCrossChainOrder(address originSettler,address user,uint256 nonce,\
uint256 originChainId,uint32 openDeadline,uint32 fillDeadline,bytes32 orderDataType,\
bytes orderData)"
        );
    }

    #[test]
    fn solvent_order_type_is_the_hash_of_its_type_string() {
        assert_eq!(
            solvent_order_type(),
            keccak256(SolventOrder::eip712_encode_type().as_bytes())
        );
    }
}
