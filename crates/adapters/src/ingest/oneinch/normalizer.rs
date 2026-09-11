//! Decodes a 1inch Limit Order Protocol v4 order into the canonical `Intent`.
//!
//! The bar for accepting an order is "can this codebase determine its real making/taking
//! amounts and construct a fill for it", not "will the fill definitely succeed" — the real
//! protocol enforces its own fillability checks on-chain regardless of what this normalizer
//! thinks, the same way a wrong-nonce UniswapX order fails at the reactor rather than here. That
//! excludes only shapes whose *amounts* depend on state this normalizer cannot evaluate, or whose
//! fill requires extra calldata this codebase does not construct:
//!
//! - A **pre-interaction** or **Permit2** flag means the fill needs data this codebase does not
//!   build — arbitrary maker-specified interaction calldata, or a Permit2 signature blob — not
//!   just a bit this normalizer could pass through unchanged, so an order carrying either is
//!   refused.
//! - An **epoch-manager check** gates *whether* the order is still live (the maker's own
//!   bulk-invalidate-by-series mechanism, `SeriesEpochManager.epochEquals`), evaluated purely
//!   from the order's own encoded `maker`/`series`/`epoch` fields against on-chain state — it
//!   requires no extra calldata from the taker and never touches `makingAmount`/`takingAmount`
//!   (confirmed against `OrderMixin.sol`'s `needCheckEpochManager` check, which runs before any
//!   transfer and only ever reverts `WrongSeriesNonce`, exactly like the `FeeTaker` whitelist
//!   check below). So it is accepted at its stated amounts, and a maker who has since
//!   invalidated the epoch is discovered the same way an unwhitelisted taker is: a real revert
//!   at fill time, not a guess made here.
//! - An **extension** field the protocol reserves for maker/taker asset suffixes, permits, a
//!   predicate, or a runtime amount calculator (making/taking amount data) can only change what
//!   this order actually costs — refused unless the extension is provably just 1inch's own
//!   `FeeTaker` post-interaction (see [`accepted_extension`]), which takes its cut from the
//!   maker's proceeds *after* the amounts below are already settled and so does not change them.
//! - **`ALLOW_MULTIPLE_FILLS`** does not touch the amounts either — a partially-filled order still
//!   asks for its full stated `makingAmount`/`takingAmount` from a taker who takes all of what
//!   remains — so it is accepted; partial-fill accounting (how much of a many-fill order is left)
//!   simply is not modeled, and every order here is treated as an all-or-nothing quote for its
//!   *stated* amounts.
//! - **An allowed-sender restriction** (the low 80 bits of `makerTraits`) restricts who may fill,
//!   not what the order is worth, so it does not gate acceptance — a wrong-sender fill fails at
//!   the reactor, not here.

use alloy::primitives::{Address, U256};
use alloy::sol_types::{SolStruct, SolValue};

use solvent_core::deps::ingest::{NormalizeError, Normalizer};
use solvent_core::primitives::ingest::{
    AmountCurve, Intent, IntentInput, IntentOutput, IntentParts, ProtocolId, RawOrder,
};
use solvent_core::primitives::IntentId;

use super::codec::{
    domain, extension_custom_data, extension_field, extension_matches_salt, fee_taker_address,
    router_address, WireOrder, POST_INTERACTION_DATA,
};

// `MakerTraits` bit positions, per `MakerTraitsLib.sol`. `ALLOW_MULTIPLE_FILLS` (bit 254) and
// `NEED_CHECK_EPOCH_MANAGER` (bit 250) are deliberately absent: this normalizer never checks
// either, per the module doc.
const PRE_INTERACTION: usize = 252;
const POST_INTERACTION: usize = 251;
const HAS_EXTENSION: usize = 249;
const USE_PERMIT2: usize = 248;
const EXPIRATION_SHIFT: usize = 80;
const EXPIRATION_BITS: usize = 40;
const ALLOWED_SENDER_BITS: usize = 80;

/// Flags refused outright: each means the fill needs calldata this codebase does not construct
/// (arbitrary pre-interaction data, a Permit2 blob), regardless of what the extension (if any)
/// turns out to contain.
fn always_unsupported_mask() -> U256 {
    [PRE_INTERACTION, USE_PERMIT2]
        .into_iter()
        .fold(U256::ZERO, |mask, bit| mask | (U256::from(1u8) << bit))
}

fn bit(traits: U256, position: usize) -> bool {
    !(traits & (U256::from(1u8) << position)).is_zero()
}

fn low_bits_mask(bits: usize) -> U256 {
    (U256::from(1u8) << bits) - U256::from(1u8)
}

/// The 40-bit expiration timestamp in bits 80..120; 0 means no expiration.
fn expiration(maker_traits: U256) -> u64 {
    let value = (maker_traits >> EXPIRATION_SHIFT) & low_bits_mask(EXPIRATION_BITS);
    let be = value.to_be_bytes::<32>();
    u64::from_be_bytes(
        be[24..32]
            .try_into()
            .expect("a 40-bit value's last 8 big-endian bytes always form a [u8; 8]"),
    )
}

/// Whether `extension` is *only* a 1inch `FeeTaker` post-interaction call for `chain` — every other
/// dynamic field empty, nothing trailing — the one extension shape whose amounts this normalizer
/// can still vouch for.
fn accepted_extension(extension: &[u8], chain: solvent_core::primitives::ChainId) -> bool {
    let Some(fee_taker) = fee_taker_address(chain) else {
        return false;
    };
    for field in 0..POST_INTERACTION_DATA {
        if !extension_field(extension, field).is_empty() {
            return false;
        }
    }
    let post_interaction = extension_field(extension, POST_INTERACTION_DATA);
    if post_interaction.len() < Address::len_bytes() {
        return false;
    }
    if Address::from_slice(&post_interaction[..Address::len_bytes()]) != fee_taker {
        return false;
    }
    extension_custom_data(extension).is_empty()
}

pub struct OneInchNormalizer;

impl Normalizer for OneInchNormalizer {
    fn normalize(&self, raw: &RawOrder) -> Result<Intent, NormalizeError> {
        let bad = || NormalizeError::Decode(ProtocolId::OneInchLimitOrder);
        let wire = WireOrder::abi_decode(&raw.payload).map_err(|_| bad())?;
        let order = wire.order;
        let extension = wire.extension;

        if !(order.makerTraits & always_unsupported_mask()).is_zero() {
            return Err(bad());
        }
        if !(order.makerTraits & low_bits_mask(ALLOWED_SENDER_BITS)).is_zero() {
            return Err(bad());
        }

        match (bit(order.makerTraits, HAS_EXTENSION), extension.is_empty()) {
            // No extension bytes at all: fine as long as the order does not claim a
            // post-interaction it then carries no data for.
            (false, true) => {
                if bit(order.makerTraits, POST_INTERACTION) {
                    return Err(bad());
                }
            }
            // Extension present: must be exactly a `FeeTaker` post-interaction, bound to this
            // order by the salt/extension-hash check the contract itself enforces.
            (true, false) => {
                if !bit(order.makerTraits, POST_INTERACTION) {
                    return Err(bad());
                }
                if !extension_matches_salt(order.salt, &extension) {
                    return Err(bad());
                }
                if !accepted_extension(&extension, raw.chain) {
                    return Err(bad());
                }
            }
            // Flag and payload disagree about whether there is an extension: not a shape the
            // contract itself would accept either (`OrderLib.isValidExtension`).
            (false, false) | (true, true) => return Err(bad()),
        }

        let router = router_address(raw.chain).ok_or_else(bad)?;
        let id = order.eip712_signing_hash(&domain(raw.chain, router));

        let receiver = if order.receiver == Address::ZERO {
            order.maker
        } else {
            order.receiver
        };

        let exp = expiration(order.makerTraits);
        let deadline = if exp == 0 { u64::MAX } else { exp };

        // 1inch calls the order signer "maker" (as in market-making a limit order); this
        // codebase calls that same role "taker" (the swapper taking liquidity from Solvent's own
        // makers) — the signer gives `makerAsset`/`makingAmount` and receives
        // `takerAsset`/`takingAmount` at `receiver`, exactly like UniswapX's swapper gives
        // `baseInput` and receives `baseOutputs` at `o.recipient`.
        Ok(Intent::new(IntentParts {
            deadline,
            exclusivity: None,
            settler: router,
            raw: raw.payload.clone(),
            signature: raw.signature.clone(),
            observed_at: raw.observed_at,
            source: raw.source,
            ..IntentParts::new(
                IntentId(id),
                ProtocolId::OneInchLimitOrder,
                order.maker,
                IntentInput::new(order.makerAsset, AmountCurve::scalar(order.makingAmount)),
                vec![IntentOutput::new(
                    order.takerAsset,
                    AmountCurve::scalar(order.takingAmount),
                    receiver,
                )],
                raw.chain,
            )
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, keccak256, Bytes};
    use solvent_core::primitives::ingest::OrderSource;
    use solvent_core::primitives::ChainId;

    use super::super::codec::Order;

    // Exercised only by these tests, which build orders setting these bits directly — production
    // code never checks either, per the module doc.
    const ALLOW_MULTIPLE_FILLS: usize = 254;
    const NEED_CHECK_EPOCH_MANAGER: usize = 250;

    const FEE_TAKER: Address = address!("c0DFdB9E7a392c3dBBE7c6FBe8FBC1789C9FE05e");

    fn order(maker_traits: U256) -> Order {
        Order {
            salt: U256::from(1u64),
            maker: address!("1111111111111111111111111111111111111111"),
            receiver: Address::ZERO,
            makerAsset: address!("2222222222222222222222222222222222222222"),
            takerAsset: address!("3333333333333333333333333333333333333333"),
            makingAmount: U256::from(1_000u64),
            takingAmount: U256::from(2_000u64),
            makerTraits: maker_traits,
        }
    }

    fn wire_payload(order: &Order, extension: Vec<u8>) -> Bytes {
        Bytes::from(
            WireOrder {
                order: order.clone(),
                extension: extension.into(),
            }
            .abi_encode(),
        )
    }

    fn raw_of(order: &Order, extension: Vec<u8>, chain: u64) -> RawOrder {
        RawOrder::new(
            ProtocolId::OneInchLimitOrder,
            ChainId(chain),
            wire_payload(order, extension),
            Bytes::from(vec![0xABu8; 65]),
            999,
            OrderSource::OneInch,
        )
    }

    fn fee_taker_extension() -> Vec<u8> {
        let post_interaction_data = FEE_TAKER.as_slice().to_vec();
        let mut offsets = U256::ZERO;
        offsets |= U256::from(post_interaction_data.len() as u64) << (32 * POST_INTERACTION_DATA);
        let mut bytes = offsets.to_be_bytes::<32>().to_vec();
        bytes.extend_from_slice(&post_interaction_data);
        bytes
    }

    /// Rewrites `salt`'s low 160 bits to bind it to `extension`, exactly as a real signer would
    /// (and as `OrderLib.isValidExtension` requires).
    fn bind_salt_to(order: &mut Order, extension: &[u8]) {
        let hash = U256::from_be_bytes(keccak256(extension).0);
        let mask = (U256::from(1u8) << 160u32) - U256::from(1u8);
        let high = order.salt & !mask;
        order.salt = high | (hash & mask);
    }

    #[test]
    fn maps_a_plain_order() {
        let o = order(U256::ZERO);
        let intent = OneInchNormalizer
            .normalize(&raw_of(&o, Vec::new(), 1))
            .expect("plain order");

        assert_eq!(intent.protocol, ProtocolId::OneInchLimitOrder);
        assert_eq!(intent.swapper, o.maker);
        assert_eq!(intent.input.token, o.makerAsset);
        assert_eq!(
            intent.input.curve,
            AmountCurve::scalar(U256::from(1_000u64))
        );
        assert_eq!(intent.outputs.len(), 1);
        assert_eq!(intent.outputs[0].token, o.takerAsset);
        assert_eq!(
            intent.outputs[0].curve,
            AmountCurve::scalar(U256::from(2_000u64))
        );
        // receiver defaults to maker when the order leaves it unset.
        assert_eq!(intent.outputs[0].recipient, o.maker);
        assert_eq!(intent.deadline, u64::MAX);
        assert_eq!(intent.exclusivity, None);
        assert_eq!(
            intent.settler,
            address!("111111125421cA6dc452d289314280a0f8842A65")
        );
        assert_eq!(
            intent.id,
            IntentId(o.eip712_signing_hash(&domain(ChainId(1), intent.settler)))
        );
        assert_eq!(intent.source, OrderSource::OneInch);
    }

    #[test]
    fn explicit_receiver_overrides_maker() {
        let mut o = order(U256::ZERO);
        o.receiver = address!("4444444444444444444444444444444444444444");
        let intent = OneInchNormalizer
            .normalize(&raw_of(&o, Vec::new(), 1))
            .expect("plain order");
        assert_eq!(intent.outputs[0].recipient, o.receiver);
    }

    #[test]
    fn nonzero_expiration_becomes_the_deadline() {
        let o = order(U256::from(1_800_000_000u64) << EXPIRATION_SHIFT);
        let intent = OneInchNormalizer
            .normalize(&raw_of(&o, Vec::new(), 1))
            .expect("plain order");
        assert_eq!(intent.deadline, 1_800_000_000);
    }

    #[test]
    fn allow_multiple_fills_is_accepted_at_its_stated_amounts() {
        let o = order(U256::from(1u8) << ALLOW_MULTIPLE_FILLS);
        let intent = OneInchNormalizer
            .normalize(&raw_of(&o, Vec::new(), 1))
            .expect("multi-fill order, still quotable at its stated amounts");
        assert_eq!(
            intent.input.curve,
            AmountCurve::scalar(U256::from(1_000u64))
        );
    }

    #[test]
    fn an_order_needing_epoch_check_is_accepted_at_its_stated_amounts() {
        let o = order(U256::from(1u8) << NEED_CHECK_EPOCH_MANAGER);
        let intent = OneInchNormalizer
            .normalize(&raw_of(&o, Vec::new(), 1))
            .expect("epoch-gated order, still quotable at its stated amounts");
        assert_eq!(
            intent.input.curve,
            AmountCurve::scalar(U256::from(1_000u64))
        );
    }

    #[test]
    fn no_partial_fills_flag_alone_is_accepted() {
        let o = order(U256::from(1u8) << 255u32);
        assert!(OneInchNormalizer
            .normalize(&raw_of(&o, Vec::new(), 1))
            .is_ok());
    }

    #[test]
    fn a_pure_fee_taker_extension_is_accepted_at_its_stated_amounts() {
        let mut o =
            order((U256::from(1u8) << HAS_EXTENSION) | (U256::from(1u8) << POST_INTERACTION));
        let extension = fee_taker_extension();
        bind_salt_to(&mut o, &extension);
        let intent = OneInchNormalizer
            .normalize(&raw_of(&o, extension, 1))
            .expect("fee-taker-gated order, still quotable at its stated amounts");
        assert_eq!(
            intent.input.curve,
            AmountCurve::scalar(U256::from(1_000u64))
        );
    }

    #[test]
    fn a_fee_taker_extension_with_wrong_salt_binding_is_rejected() {
        let o = order((U256::from(1u8) << HAS_EXTENSION) | (U256::from(1u8) << POST_INTERACTION));
        let extension = fee_taker_extension(); // salt left as `order()`'s default: does not match
        assert!(matches!(
            OneInchNormalizer.normalize(&raw_of(&o, extension, 1)),
            Err(NormalizeError::Decode(ProtocolId::OneInchLimitOrder))
        ));
    }

    #[test]
    fn an_extension_targeting_an_unknown_contract_is_rejected() {
        let mut o =
            order((U256::from(1u8) << HAS_EXTENSION) | (U256::from(1u8) << POST_INTERACTION));
        let not_fee_taker = address!("9999999999999999999999999999999999999999");
        let post_interaction_data = not_fee_taker.as_slice().to_vec();
        let mut offsets = U256::ZERO;
        offsets |= U256::from(post_interaction_data.len() as u64) << (32 * POST_INTERACTION_DATA);
        let mut extension = offsets.to_be_bytes::<32>().to_vec();
        extension.extend_from_slice(&post_interaction_data);
        bind_salt_to(&mut o, &extension);
        assert!(matches!(
            OneInchNormalizer.normalize(&raw_of(&o, extension, 1)),
            Err(NormalizeError::Decode(ProtocolId::OneInchLimitOrder))
        ));
    }

    #[test]
    fn an_extension_with_a_populated_predicate_is_rejected() {
        // Field 4 (Predicate) nonempty alongside field 7 (PostInteractionData): the runtime
        // condition it encodes could gate or shape the fill in a way this normalizer cannot
        // evaluate, so the whole order is refused even though PostInteractionData alone would
        // have passed.
        let mut o =
            order((U256::from(1u8) << HAS_EXTENSION) | (U256::from(1u8) << POST_INTERACTION));
        let predicate = vec![0xEEu8; 4];
        let post_interaction_data = FEE_TAKER.as_slice().to_vec();
        let mut offsets = U256::ZERO;
        offsets |= U256::from(predicate.len() as u64) << (32 * 4);
        for i in 5..POST_INTERACTION_DATA {
            offsets |= U256::from(predicate.len() as u64) << (32 * i);
        }
        offsets |= U256::from((predicate.len() + post_interaction_data.len()) as u64)
            << (32 * POST_INTERACTION_DATA);
        let mut extension = offsets.to_be_bytes::<32>().to_vec();
        extension.extend_from_slice(&predicate);
        extension.extend_from_slice(&post_interaction_data);
        bind_salt_to(&mut o, &extension);
        assert!(matches!(
            OneInchNormalizer.normalize(&raw_of(&o, extension, 1)),
            Err(NormalizeError::Decode(ProtocolId::OneInchLimitOrder))
        ));
    }

    #[test]
    fn rejects_an_order_needing_pre_interaction() {
        let o = order(U256::from(1u8) << PRE_INTERACTION);
        assert!(matches!(
            OneInchNormalizer.normalize(&raw_of(&o, Vec::new(), 1)),
            Err(NormalizeError::Decode(ProtocolId::OneInchLimitOrder))
        ));
    }

    #[test]
    fn rejects_an_order_using_permit2() {
        let o = order(U256::from(1u8) << USE_PERMIT2);
        assert!(matches!(
            OneInchNormalizer.normalize(&raw_of(&o, Vec::new(), 1)),
            Err(NormalizeError::Decode(ProtocolId::OneInchLimitOrder))
        ));
    }

    #[test]
    fn rejects_a_post_interaction_flag_with_no_extension_bytes() {
        let o = order(U256::from(1u8) << POST_INTERACTION);
        assert!(matches!(
            OneInchNormalizer.normalize(&raw_of(&o, Vec::new(), 1)),
            Err(NormalizeError::Decode(ProtocolId::OneInchLimitOrder))
        ));
    }

    #[test]
    fn rejects_an_allowed_sender_restriction() {
        let sender = address!("5555555555555555555555555555555555555555");
        let mut traits = [0u8; 32];
        traits[12..32].copy_from_slice(sender.as_slice());
        let o = order(U256::from_be_bytes(traits));
        assert!(matches!(
            OneInchNormalizer.normalize(&raw_of(&o, Vec::new(), 1)),
            Err(NormalizeError::Decode(ProtocolId::OneInchLimitOrder))
        ));
    }

    #[test]
    fn rejects_an_unsupported_chain() {
        let o = order(U256::ZERO);
        assert!(matches!(
            OneInchNormalizer.normalize(&raw_of(&o, Vec::new(), 10)),
            Err(NormalizeError::Decode(ProtocolId::OneInchLimitOrder))
        ));
    }

    #[test]
    fn rejects_a_malformed_payload() {
        let raw = RawOrder::new(
            ProtocolId::OneInchLimitOrder,
            ChainId(1),
            Bytes::from_static(&[1, 2, 3]),
            Bytes::new(),
            0,
            OrderSource::OneInch,
        );
        assert!(matches!(
            OneInchNormalizer.normalize(&raw),
            Err(NormalizeError::Decode(ProtocolId::OneInchLimitOrder))
        ));
    }
}
