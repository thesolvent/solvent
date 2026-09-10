//! Decodes a UniswapX V2 Dutch order into the canonical `Intent`, and refuses anything the reactor
//! would revert on or that this filler cannot settle. Resolved amounts map onto `AmountCurve::dutch`,
//! whose decay mirrors `DutchDecayLib` bit-for-bit.
//!
//! Every rejection here is an order we would otherwise pay gas to discover was unfillable. Orders
//! arrive from a public API and are attacker-shaped, so the checks are an allow-list of what we can
//! settle, not a deny-list of what we have seen fail.

use alloy::primitives::{Address, Signature, B256, U256};
use alloy::sol_types::SolValue;

use solvent_core::deps::ingest::{NormalizeError, Normalizer};
use solvent_core::primitives::ingest::{
    AmountCurve, Exclusivity, Intent, IntentInput, IntentOutput, IntentParts, ProtocolId, RawOrder,
};
use solvent_core::primitives::IntentId;

use super::builder::cosign_digest;
use super::codec::{order_hash, V2DutchOrder};

/// Decodes and validates V2 Dutch orders against the deployment this resolver fills for.
pub struct UniswapXV2Normalizer {
    reactor: Address,
    /// Cosigner keys whose signature we accept. Uniswap operates one per deployment and can rotate
    /// it; the reactor itself checks only that the signature matches the order's own `cosigner`
    /// field, so pinning the identity is ours to do.
    cosigners: Vec<Address>,
}

impl UniswapXV2Normalizer {
    pub fn new(reactor: Address, cosigners: Vec<Address>) -> UniswapXV2Normalizer {
        UniswapXV2Normalizer { reactor, cosigners }
    }
}

impl Normalizer for UniswapXV2Normalizer {
    fn normalize(&self, raw: &RawOrder) -> Result<Intent, NormalizeError> {
        let bad = || NormalizeError::Decode(ProtocolId::UniswapXV2);
        let order = V2DutchOrder::abi_decode(&raw.payload).map_err(|_| bad())?;
        let cd = &order.cosignerData;
        if order.info.reactor != self.reactor {
            return Err(bad());
        }
        // An arbitrary contract the reactor calls inside our fill, with our address as an argument:
        // it can burn unbounded gas or revert on state an `eth_call` cannot reproduce, turning a
        // simulation into a false positive. We do not fill orders that carry one.
        if order.info.additionalValidationContract != Address::ZERO {
            return Err(bad());
        }
        // The reactor requires one cosigner override per output; a mismatch is a malformed order.
        if cd.outputAmounts.len() != order.baseOutputs.len() {
            return Err(bad());
        }
        // Overrides may only improve on what the swapper signed, and the reactor reverts otherwise.
        if !cd.inputAmount.is_zero() && cd.inputAmount > order.baseInput.startAmount {
            return Err(bad());
        }
        let overrides_within_bounds = cd
            .outputAmounts
            .iter()
            .zip(order.baseOutputs.iter())
            .all(|(o, base)| o.is_zero() || *o >= base.startAmount);
        if !overrides_within_bounds {
            return Err(bad());
        }
        if order.info.deadline < cd.decayEndTime {
            return Err(bad());
        }
        // `DutchDecayLib` reverts `EndTimeBeforeStartTime` on a window that does not advance, and
        // `IncorrectAmounts` on a curve that decays the wrong way — an output may only fall and an
        // input may only rise. Resolving these off-chain would price an order the reactor refuses.
        if cd.decayEndTime <= cd.decayStartTime {
            return Err(bad());
        }
        if order.baseInput.endAmount < order.baseInput.startAmount {
            return Err(bad());
        }
        if order
            .baseOutputs
            .iter()
            .any(|o| o.endAmount > o.startAmount)
        {
            return Err(bad());
        }
        verify_cosignature(&order, &self.cosigners).ok_or_else(bad)?;

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

        // The window ends when the decay begins — the reactor passes `decayStartTime` as its
        // exclusivity end, there is no separate field.
        let exclusivity = (cd.exclusiveFiller != Address::ZERO)
            .then(|| {
                u16::try_from(cd.exclusivityOverrideBps)
                    .map(|bps| Exclusivity::new(cd.exclusiveFiller, start_time, bps))
            })
            .transpose()
            .map_err(|_| bad())?;

        Ok(Intent::new(IntentParts {
            deadline: u64::try_from(order.info.deadline).map_err(|_| bad())?,
            exclusivity,
            settler: order.info.reactor,
            raw: raw.payload.clone(),
            signature: raw.signature.clone(),
            observed_at: raw.observed_at,
            ..IntentParts::new(
                IntentId(order_hash(&order)),
                ProtocolId::UniswapXV2,
                order.info.swapper,
                input,
                outputs,
                raw.chain,
            )
        }))
    }
}

/// A cosigner override replaces the base amount only when non-zero (0 means "no override").
fn overridden(override_amount: U256, base: U256) -> U256 {
    Some(override_amount)
        .filter(|a| !a.is_zero())
        .unwrap_or(base)
}

/// Recovers the cosignature over `keccak256(orderHash ‖ abi.encode(cosignerData))` — the reactor's
/// own digest — and accepts it only from a pinned cosigner. `None` on any failure; the caller maps
/// that to a decode error, since a caller can do nothing but drop the order either way.
fn verify_cosignature(order: &V2DutchOrder, cosigners: &[Address]) -> Option<()> {
    let hash: B256 = order_hash(order);
    // `from_raw` normalises the recovery byte, but the reactor hands `cosignature[64]` straight to
    // `ecrecover`, which needs 27 or 28. Accepting 0/1 would admit an order that reverts on chain.
    if !matches!(order.cosignature.get(64), Some(27 | 28)) {
        return None;
    }
    let signature = Signature::from_raw(&order.cosignature).ok()?;
    let signer = signature
        .recover_address_from_prehash(&cosign_digest(order, hash))
        .ok()?;
    (signer == order.cosigner && cosigners.contains(&signer)).then_some(())
}

#[cfg(test)]
mod tests {
    use super::super::builder::{sign65, OrderSpec, SignedOrderBuilder};
    use super::super::codec::V2DutchOrder;
    use super::*;
    use alloy::primitives::{address, Bytes};
    use alloy::signers::local::PrivateKeySigner;
    use alloy::sol_types::SolValue;
    use solvent_core::primitives::ChainId;

    const PERMIT2: Address = address!("000000000022D473030F116dDEE9F6B43aC78BA3");
    const REACTOR: Address = address!("2222222222222222222222222222222222222222");

    fn key(byte: u8) -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::from([byte; 32])).expect("valid test key")
    }

    fn spec() -> OrderSpec {
        OrderSpec {
            reactor: REACTOR,
            nonce: U256::from(1u64),
            deadline: 5000,
            input_token: address!("6666666666666666666666666666666666666666"),
            input_start: U256::from(500u64),
            input_end: U256::from(500u64),
            output_token: address!("7777777777777777777777777777777777777777"),
            output_start: U256::from(1000u64),
            output_end: U256::from(900u64),
            recipient: address!("8888888888888888888888888888888888888888"),
            decay_start: 1000,
            decay_end: 1100,
            exclusive_filler: address!("5555555555555555555555555555555555555555"),
            exclusivity_override_bps: 100,
        }
    }

    /// A cosigned order, plus the normalizer that trusts the key which cosigned it.
    fn fixture() -> (RawOrder, UniswapXV2Normalizer) {
        let builder = SignedOrderBuilder::new(PERMIT2, 1, key(0x11), key(0x22));
        let raw = builder.build(&spec(), 1234);
        let normalizer = UniswapXV2Normalizer::new(REACTOR, vec![key(0x22).address()]);
        (raw, normalizer)
    }

    /// An order the trusted cosigner really did cosign, with `mutate` applied first.
    ///
    /// Re-cosigning is the whole point: almost every field feeds the order hash or the cosigner
    /// digest, so mutating one and keeping the old signature makes the signature check reject the
    /// order whatever else is wrong with it — and a test named after some other rule would pass
    /// with that rule deleted.
    fn tampered(mutate: impl FnOnce(&mut V2DutchOrder)) -> RawOrder {
        reencoded(mutate, Recosign::Yes)
    }

    /// An order altered downstream of the cosigner: the fields move, the cosignature does not.
    fn forged(mutate: impl FnOnce(&mut V2DutchOrder)) -> RawOrder {
        reencoded(mutate, Recosign::No)
    }

    enum Recosign {
        Yes,
        No,
    }

    fn reencoded(mutate: impl FnOnce(&mut V2DutchOrder), recosign: Recosign) -> RawOrder {
        let (raw, _) = fixture();
        let mut order = V2DutchOrder::abi_decode(&raw.payload).expect("decode");
        mutate(&mut order);
        if let Recosign::Yes = recosign {
            // Blanked first, as the builder does, so the digest is over the order without it.
            order.cosignature = Bytes::new();
            let hash = order_hash(&order);
            order.cosignature = sign65(&key(0x22), cosign_digest(&order, hash));
        }
        RawOrder::new(
            ProtocolId::UniswapXV2,
            ChainId(1),
            Bytes::from(order.abi_encode()),
            raw.signature.clone(),
            raw.observed_at,
        )
    }

    fn rejects(raw: &RawOrder) {
        let (_, normalizer) = fixture();
        assert!(matches!(
            normalizer.normalize(raw),
            Err(NormalizeError::Decode(_))
        ));
    }

    #[test]
    fn normalizes_a_cosigned_order() {
        let (raw, normalizer) = fixture();
        let intent = normalizer.normalize(&raw).expect("normalizes");
        let spec = spec();
        assert_eq!(intent.swapper, key(0x11).address());
        assert_eq!(intent.settler, REACTOR);
        assert_eq!(
            intent.exclusivity,
            Some(Exclusivity::new(
                spec.exclusive_filler,
                spec.decay_start,
                100
            ))
        );
    }

    /// The exclusivity window is closed at its own end instant and open one second later — the
    /// reactor compares strictly greater.
    #[test]
    fn exclusivity_window_is_closed_at_its_end() {
        let (raw, normalizer) = fixture();
        let intent = normalizer.normalize(&raw).expect("normalizes");
        let ex = intent.exclusivity.expect("exclusive");
        let other = Address::repeat_byte(0xAB);
        assert!(!ex.grants_rights_to(other, ex.ends_at));
        assert!(ex.grants_rights_to(other, ex.ends_at + 1));
        assert!(ex.grants_rights_to(ex.filler, ex.ends_at));
    }

    #[test]
    fn rejects_a_foreign_cosigner() {
        let (raw, _) = fixture();
        let stranger = UniswapXV2Normalizer::new(REACTOR, vec![key(0x33).address()]);
        assert!(matches!(
            stranger.normalize(&raw),
            Err(NormalizeError::Decode(_))
        ));
    }

    #[test]
    fn rejects_a_tampered_cosigner_payload() {
        // The exclusive filler is in the cosigner's digest and in no other rule, so only the
        // signature check can object to it moving.
        rejects(&forged(|o| {
            o.cosignerData.exclusiveFiller = Address::repeat_byte(0xAB)
        }));
    }

    #[test]
    fn rejects_another_deployments_reactor() {
        rejects(&tampered(|o| o.info.reactor = Address::repeat_byte(0x99)));
    }

    #[test]
    fn rejects_an_additional_validation_hook() {
        rejects(&tampered(|o| {
            o.info.additionalValidationContract = Address::repeat_byte(0x77)
        }));
    }

    /// The reactor requires one override per output and reverts on a mismatch.
    #[test]
    fn rejects_cosigner_output_length_mismatch() {
        rejects(&tampered(|o| o.cosignerData.outputAmounts.clear()));
    }

    /// `InvalidCosignerInput`: an override may only lower what the swapper signed.
    #[test]
    fn rejects_an_input_override_above_the_signed_start() {
        rejects(&tampered(|o| {
            o.cosignerData.inputAmount = o.baseInput.startAmount + U256::from(1u64)
        }));
    }

    /// `InvalidCosignerOutput`: an override may only raise what the swapper signed.
    #[test]
    fn rejects_an_output_override_below_the_signed_start() {
        rejects(&tampered(|o| {
            o.cosignerData.outputAmounts[0] = o.baseOutputs[0].startAmount - U256::from(1u64)
        }));
    }

    /// `EndTimeBeforeStartTime`: a window that does not advance.
    #[test]
    fn rejects_a_decay_window_that_does_not_advance() {
        rejects(&tampered(|o| {
            o.cosignerData.decayEndTime = o.cosignerData.decayStartTime
        }));
        rejects(&tampered(|o| {
            o.cosignerData.decayEndTime = o.cosignerData.decayStartTime - U256::from(1u64)
        }));
    }

    /// `IncorrectAmounts`: an output may only fall.
    #[test]
    fn rejects_an_output_that_decays_upward() {
        rejects(&tampered(|o| {
            o.baseOutputs[0].endAmount = o.baseOutputs[0].startAmount + U256::from(1u64)
        }));
    }

    /// `IncorrectAmounts`: an input may only rise.
    #[test]
    fn rejects_an_input_that_decays_downward() {
        rejects(&tampered(|o| {
            o.baseInput.endAmount = o.baseInput.startAmount - U256::from(1u64)
        }));
    }

    /// The reactor feeds `cosignature[64]` to `ecrecover`, which needs 27/28. A genuine signature
    /// with the recovery byte rewritten to 0/1 still recovers locally and reverts on chain.
    #[test]
    fn rejects_a_cosignature_with_a_raw_recovery_byte() {
        let (raw, normalizer) = fixture();
        let mut order = V2DutchOrder::abi_decode(&raw.payload).expect("decode");
        let mut sig = order.cosignature.to_vec();
        assert!(matches!(sig[64], 27 | 28), "fixture is well formed");
        sig[64] -= 27;
        order.cosignature = Bytes::from(sig);
        let forged = RawOrder::new(
            ProtocolId::UniswapXV2,
            ChainId(1),
            Bytes::from(order.abi_encode()),
            raw.signature.clone(),
            raw.observed_at,
        );
        assert!(matches!(
            normalizer.normalize(&forged),
            Err(NormalizeError::Decode(_))
        ));
    }

    /// `DeadlineBeforeEndTime`: the decay may not outlive the order.
    #[test]
    fn rejects_a_deadline_before_the_decay_ends() {
        rejects(&tampered(|o| o.info.deadline = U256::from(1u64)));
    }
}
