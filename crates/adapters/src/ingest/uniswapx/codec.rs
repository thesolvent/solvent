//! Shared UniswapX V2 wire types and `orderHash`. The hash reproduces `V2DutchOrderLib.hash`: its
//! type string flattens `baseInput`, so alloy's derived hashing can't produce it — we compose it
//! from the contract's own type strings and alloy's `abi_encode`.

use alloy::primitives::{keccak256, B256};
use alloy::sol;
use alloy::sol_types::SolValue;

sol! {
    struct OrderInfo {
        address reactor;
        address swapper;
        uint256 nonce;
        uint256 deadline;
        address additionalValidationContract;
        bytes additionalValidationData;
    }
    struct DutchInput {
        address token;
        uint256 startAmount;
        uint256 endAmount;
    }
    struct DutchOutput {
        address token;
        uint256 startAmount;
        uint256 endAmount;
        address recipient;
    }
    struct CosignerData {
        uint256 decayStartTime;
        uint256 decayEndTime;
        address exclusiveFiller;
        uint256 exclusivityOverrideBps;
        uint256 inputAmount;
        uint256[] outputAmounts;
    }
    struct V2DutchOrder {
        OrderInfo info;
        address cosigner;
        DutchInput baseInput;
        DutchOutput[] baseOutputs;
        CosignerData cosignerData;
        bytes cosignature;
    }
}

// EIP-712 type strings, verbatim from the UniswapX libs — their exact bytes drive `orderHash` and
// the swapper's Permit2 witness type string (the builder).
pub(crate) const ORDER_INFO_TYPE: &[u8] = b"OrderInfo(address reactor,address swapper,uint256 nonce,uint256 deadline,address additionalValidationContract,bytes additionalValidationData)";
pub(crate) const DUTCH_OUTPUT_TYPE: &[u8] =
    b"DutchOutput(address token,uint256 startAmount,uint256 endAmount,address recipient)";
pub(crate) const V2_DUTCH_ORDER_TYPE: &[u8] = b"V2DutchOrder(OrderInfo info,address cosigner,address baseInputToken,uint256 baseInputStartAmount,uint256 baseInputEndAmount,DutchOutput[] baseOutputs)";

/// `V2DutchOrderLib.hash` over the pre-cosigner-override base amounts — the hash the swapper signed.
pub(crate) fn order_hash(order: &V2DutchOrder) -> B256 {
    let order_type_hash =
        keccak256([V2_DUTCH_ORDER_TYPE, DUTCH_OUTPUT_TYPE, ORDER_INFO_TYPE].concat());
    keccak256(
        (
            order_type_hash,
            info_hash(&order.info),
            order.cosigner,
            order.baseInput.token,
            order.baseInput.startAmount,
            order.baseInput.endAmount,
            outputs_hash(&order.baseOutputs),
        )
            .abi_encode(),
    )
}

/// `OrderInfoLib.hash` — the `bytes` field is hashed, per EIP-712.
fn info_hash(info: &OrderInfo) -> B256 {
    keccak256(
        (
            keccak256(ORDER_INFO_TYPE),
            info.reactor,
            info.swapper,
            info.nonce,
            info.deadline,
            info.additionalValidationContract,
            keccak256(&info.additionalValidationData),
        )
            .abi_encode(),
    )
}

/// `DutchOrderLib.hash(DutchOutput[])` — keccak of the packed per-output struct hashes.
fn outputs_hash(outputs: &[DutchOutput]) -> B256 {
    let type_hash = keccak256(DUTCH_OUTPUT_TYPE);
    let packed: Vec<u8> = outputs
        .iter()
        .flat_map(|o| {
            keccak256((type_hash, o.token, o.startAmount, o.endAmount, o.recipient).abi_encode()).0
        })
        .collect();
    keccak256(packed)
}
