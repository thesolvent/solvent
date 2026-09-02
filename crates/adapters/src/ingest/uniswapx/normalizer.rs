//! Decodes a UniswapX V2 Dutch order into the canonical `Intent`. The `orderHash` (our `IntentId`)
//! reproduces `V2DutchOrderLib.hash` from alloy's `abi_encode` plus the contract's own EIP-712 type
//! strings — a custom struct hash that flattens `baseInput` into three fields, which alloy's derived
//! hashing can't produce. The resolved amounts map straight onto `AmountCurve::dutch`, whose rounding
//! mirrors `DutchDecayLib` bit-for-bit.

use alloy::primitives::{keccak256, Address, B256, U256};
use alloy::sol;
use alloy::sol_types::SolValue;

use solvent_core::deps::ingest::{NormalizeError, Normalizer};
use solvent_core::primitives::ingest::{
    AmountCurve, Exclusivity, Intent, IntentInput, IntentOutput, ProtocolId, RawOrder,
};
use solvent_core::primitives::IntentId;

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

// EIP-712 type strings, verbatim from the UniswapX libs (their exact bytes drive `orderHash`).
const ORDER_INFO_TYPE: &[u8] = b"OrderInfo(address reactor,address swapper,uint256 nonce,uint256 deadline,address additionalValidationContract,bytes additionalValidationData)";
const DUTCH_OUTPUT_TYPE: &[u8] =
    b"DutchOutput(address token,uint256 startAmount,uint256 endAmount,address recipient)";
const V2_DUTCH_ORDER_TYPE: &[u8] = b"V2DutchOrder(OrderInfo info,address cosigner,address baseInputToken,uint256 baseInputStartAmount,uint256 baseInputEndAmount,DutchOutput[] baseOutputs)";

pub struct UniswapXV2Normalizer;

impl Normalizer for UniswapXV2Normalizer {
    fn normalize(&self, raw: &RawOrder) -> Result<Intent, NormalizeError> {
        let bad = || NormalizeError::Decode(ProtocolId::UniswapXV2);
        let order = V2DutchOrder::abi_decode(&raw.payload).map_err(|_| bad())?;
        let cd = &order.cosignerData;
        let start_time = u64::try_from(cd.decayStartTime).map_err(|_| bad())?;
        let end_time = u64::try_from(cd.decayEndTime).map_err(|_| bad())?;

        let input = IntentInput::new(
            order.baseInput.token,
            AmountCurve::dutch(
                overridden(cd.inputAmount, order.baseInput.startAmount),
                order.baseInput.endAmount,
                start_time,
                end_time,
            ),
        );

        let outputs = order
            .baseOutputs
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let start = overridden(
                    cd.outputAmounts.get(i).copied().unwrap_or(U256::ZERO),
                    o.startAmount,
                );
                IntentOutput::new(
                    o.token,
                    AmountCurve::dutch(start, o.endAmount, start_time, end_time),
                    o.recipient,
                )
            })
            .collect();

        let exclusivity = (cd.exclusiveFiller != Address::ZERO).then_some(Exclusivity {
            filler: cd.exclusiveFiller,
            ends_at: start_time,
        });

        Ok(Intent::new(
            IntentId(order_hash(&order)),
            ProtocolId::UniswapXV2,
            input,
            outputs,
            u64::try_from(order.info.deadline).map_err(|_| bad())?,
            exclusivity,
            order.info.reactor,
            raw.chain,
            raw.payload.clone(),
            raw.observed_at,
        ))
    }
}

/// A cosigner override replaces the base amount only when non-zero (0 means "no override").
fn overridden(override_amount: U256, base: U256) -> U256 {
    Some(override_amount)
        .filter(|a| !a.is_zero())
        .unwrap_or(base)
}

/// `V2DutchOrderLib.hash(order)` — `keccak256(abi.encode(ORDER_TYPE_HASH, info.hash(), cosigner,
/// baseInput.token, baseInput.startAmount, baseInput.endAmount, baseOutputs.hash()))`, over the
/// pre-cosigner-override base amounts (the hash the swapper signed). The `abi.encode` is alloy's; we
/// supply only the flattened type string, which alloy's derived hashing can't reproduce.
fn order_hash(order: &V2DutchOrder) -> B256 {
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
