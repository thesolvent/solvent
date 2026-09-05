//! Decodes a UniswapX V2 Dutch order into the canonical `Intent`. Resolved amounts map onto
//! `AmountCurve::dutch`, whose rounding mirrors `DutchDecayLib` bit-for-bit.

use alloy::primitives::{Address, U256};
use alloy::sol_types::SolValue;

use solvent_core::deps::ingest::{NormalizeError, Normalizer};
use solvent_core::primitives::ingest::{
    AmountCurve, Exclusivity, Intent, IntentInput, IntentOutput, ProtocolId, RawOrder,
};
use solvent_core::primitives::IntentId;

use super::codec::{order_hash, V2DutchOrder};

pub struct UniswapXV2Normalizer;

impl Normalizer for UniswapXV2Normalizer {
    fn normalize(&self, raw: &RawOrder) -> Result<Intent, NormalizeError> {
        let bad = || NormalizeError::Decode(ProtocolId::UniswapXV2);
        let order = V2DutchOrder::abi_decode(&raw.payload).map_err(|_| bad())?;
        let cd = &order.cosignerData;
        // The reactor requires one cosigner override per output; a mismatch is a malformed order.
        if cd.outputAmounts.len() != order.baseOutputs.len() {
            return Err(bad());
        }
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

        let exclusivity = (cd.exclusiveFiller != Address::ZERO)
            .then(|| Exclusivity::new(cd.exclusiveFiller, start_time));

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
            raw.signature.clone(),
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

#[cfg(test)]
mod tests {
    use super::super::codec::{CosignerData, DutchInput, DutchOutput, OrderInfo, V2DutchOrder};
    use super::*;
    use alloy::primitives::Bytes;
    use solvent_core::primitives::ChainId;

    // One output but no cosigner override amounts — the reactor reverts on the length mismatch, so
    // ingest must reject it rather than pass a would-revert order downstream.
    #[test]
    fn rejects_cosigner_output_length_mismatch() {
        let z = Address::ZERO;
        let one = U256::from(1u64);
        let order = V2DutchOrder {
            info: OrderInfo {
                reactor: z,
                swapper: z,
                nonce: U256::ZERO,
                deadline: U256::from(100u64),
                additionalValidationContract: z,
                additionalValidationData: Bytes::new(),
            },
            cosigner: z,
            baseInput: DutchInput {
                token: z,
                startAmount: one,
                endAmount: one,
            },
            baseOutputs: vec![DutchOutput {
                token: z,
                startAmount: one,
                endAmount: one,
                recipient: z,
            }],
            cosignerData: CosignerData {
                decayStartTime: U256::ZERO,
                decayEndTime: one,
                exclusiveFiller: z,
                exclusivityOverrideBps: U256::ZERO,
                inputAmount: U256::ZERO,
                outputAmounts: vec![],
            },
            cosignature: Bytes::new(),
        };
        let raw = RawOrder::new(
            ProtocolId::UniswapXV2,
            ChainId(1),
            Bytes::from(order.abi_encode()),
            Bytes::new(),
            0,
        );
        assert!(matches!(
            UniswapXV2Normalizer.normalize(&raw),
            Err(NormalizeError::Decode(_))
        ));
    }
}
