//! 1inch Limit Order Protocol v4 wire types (`IOrderMixin.Order`), the EIP-712 domain the deployed
//! router signs orders under, and the `extension` byte format `MakerTraits.HAS_EXTENSION` orders
//! carry alongside it.
//!
//! `maker`/`receiver`/`makerAsset`/`takerAsset` are the protocol's `Address` user-defined type (a
//! flag-bearing `uint256` wrapper in Solidity), but for an order with no flags set the ABI and
//! EIP-712 encoding of `Address` and plain `address` are byte-identical, so a plain `address` field
//! decodes and hashes correctly here regardless.
//!
//! `WireOrder` is this codebase's own encoding, not 1inch's calldata shape: the feed always packs
//! an order plus its extension bytes (empty when the order carries none) so the normalizer never
//! has to guess which shape a payload is in.

use alloy::primitives::{address, keccak256, Address, U256};
use alloy::sol;
use alloy::sol_types::Eip712Domain;

use solvent_core::primitives::ChainId;

sol! {
    // Named bare `Order` — `sol!` derives the EIP-712 type string from this Rust name, and it
    // must match `OrderLib.sol`'s literal `Order(...)` typehash exactly.
    struct Order {
        uint256 salt;
        address maker;
        address receiver;
        address makerAsset;
        address takerAsset;
        uint256 makingAmount;
        uint256 takingAmount;
        uint256 makerTraits;
    }

    struct WireOrder {
        Order order;
        bytes extension;
    }
}

/// The deployed Limit Order Protocol / Aggregation Router V6 address per chain — verified against
/// the live contract's own `eip712Domain()` (EIP-5267), not assumed from docs.
pub(crate) fn router_address(chain: ChainId) -> Option<Address> {
    match chain.0 {
        1 => Some(address!("111111125421cA6dc452d289314280a0f8842A65")),
        _ => None,
    }
}

/// The EIP-712 domain the router signs orders under, confirmed on-chain via `eip712Domain()`:
/// name "1inch Aggregation Router", version "6" — not the "1inch Limit Order Protocol" / "4" name
/// the standalone `LimitOrderProtocol.sol` deployment uses; this router merged the two.
pub(crate) fn domain(chain: ChainId, router: Address) -> Eip712Domain {
    Eip712Domain::new(
        Some("1inch Aggregation Router".into()),
        Some("6".into()),
        Some(U256::from(chain.0)),
        Some(router),
        None,
    )
}

/// 1inch's own `FeeTaker` post-interaction contract, per chain — confirmed live (the address
/// embedded in a real mainnet order's extension bytes, matching the deployment 1inch's resolver
/// integration docs name). Gates liquid-pair orders to a taker whitelist, but does not change the
/// order's stated `makingAmount`/`takingAmount` — the fee comes out of the maker's proceeds inside
/// the post-interaction call, after the swap this codebase's `Intent` already prices.
pub(crate) fn fee_taker_address(chain: ChainId) -> Option<Address> {
    match chain.0 {
        1 => Some(address!("c0DFdB9E7a392c3dBBE7c6FBe8FBC1789C9FE05e")),
        _ => None,
    }
}

/// `ExtensionLib.DynamicField::PostInteractionData`'s index in the packed `offsets` header — the
/// only field this codebase reads by name; fields 0..6 are only ever checked for emptiness.
pub(crate) const POST_INTERACTION_DATA: u32 = 7;

/// One field's byte range within `extension`'s concatenated body (the part after the 32-byte
/// offsets header), per `OffsetsLib.get`: field `i`'s cumulative end offset sits in bits
/// `[32*i, 32*i+32)` of the header, and field `i` begins where field `i-1` ended.
fn field_range(offsets: U256, index: u32) -> (usize, usize) {
    let end = |i: u32| -> usize {
        let word = (offsets >> (32 * i)) & U256::from(u32::MAX);
        word.to::<u64>() as usize
    };
    let begin = if index == 0 { 0 } else { end(index - 1) };
    (begin, end(index))
}

/// Slices one dynamic field out of `extension` — empty when `extension` is too short to carry an
/// offsets header at all (an extension this codebase never produces itself, but a malformed one
/// from the wire must not panic).
pub(crate) fn extension_field(extension: &[u8], index: u32) -> &[u8] {
    if extension.len() < 32 {
        return &[];
    }
    let offsets = U256::from_be_slice(&extension[..32]);
    let body = &extension[32..];
    let (begin, end) = field_range(offsets, index);
    if end > body.len() || begin > end {
        return &[];
    }
    &body[begin..end]
}

/// The bytes beyond every named field, per `ExtensionLib.customData` (`offsets >> 224`).
pub(crate) fn extension_custom_data(extension: &[u8]) -> &[u8] {
    if extension.len() < 32 {
        return &[];
    }
    let offsets = U256::from_be_slice(&extension[..32]);
    let body = &extension[32..];
    let start = ((offsets >> 224u32) & U256::from(u32::MAX)).to::<u64>() as usize;
    if start > body.len() {
        return &[];
    }
    &body[start..]
}

/// `OrderLib.isValidExtension`: the extension's hash, low 160 bits, must equal the order's own
/// salt, low 160 bits — the contract's own binding between an order and the extension a taker
/// presents alongside it. A mismatch means the extension bytes we were handed do not belong to
/// this order.
pub(crate) fn extension_matches_salt(salt: U256, extension: &[u8]) -> bool {
    let hash = U256::from_be_bytes(keccak256(extension).0);
    let mask = (U256::from(1u8) << 160u32) - U256::from(1u8);
    (hash & mask) == (salt & mask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolStruct;

    // Pinned against `OrderLib.sol`'s `_LIMIT_ORDER_TYPEHASH` literal — a reordered or renamed
    // field would silently diverge from the deployed contract's hash.
    #[test]
    fn eip712_type_string_matches_order_lib() {
        assert_eq!(
            Order::eip712_encode_type(),
            "Order(uint256 salt,address maker,address receiver,address makerAsset,\
address takerAsset,uint256 makingAmount,uint256 takingAmount,uint256 makerTraits)"
        );
    }

    #[test]
    fn router_address_known_only_on_mainnet() {
        assert_eq!(
            router_address(ChainId(1)),
            Some(address!("111111125421cA6dc452d289314280a0f8842A65"))
        );
        assert_eq!(router_address(ChainId(10)), None);
    }

    #[test]
    fn fee_taker_address_known_only_on_mainnet() {
        assert_eq!(
            fee_taker_address(ChainId(1)),
            Some(address!("c0DFdB9E7a392c3dBBE7c6FBe8FBC1789C9FE05e"))
        );
        assert_eq!(fee_taker_address(ChainId(10)), None);
    }

    /// Builds a well-formed extension carrying only a post-interaction field, per
    /// `ExtensionLib`'s packed-offsets format: fields 0..6 empty, field 7 (`PostInteractionData`)
    /// holds `data`, nothing beyond it.
    fn post_interaction_only_extension(data: &[u8]) -> Vec<u8> {
        let end = data.len() as u64;
        // Every field's cumulative end equals `end` once we are at or past index 6 (post-interaction
        // is index 7; fields 0..6 are all zero-length, so their end offsets are all 0).
        let mut offsets = U256::ZERO;
        offsets |= U256::from(end) << (32 * POST_INTERACTION_DATA);
        let mut bytes = offsets.to_be_bytes::<32>().to_vec();
        bytes.extend_from_slice(data);
        bytes
    }

    #[test]
    fn extension_field_reads_back_a_post_interaction_only_blob() {
        let data = [0xABu8; 24];
        let ext = post_interaction_only_extension(&data);
        for i in 0..POST_INTERACTION_DATA {
            assert!(
                extension_field(&ext, i).is_empty(),
                "field {i} should be empty"
            );
        }
        assert_eq!(extension_field(&ext, POST_INTERACTION_DATA), &data);
        assert!(extension_custom_data(&ext).is_empty());
    }

    #[test]
    fn extension_field_on_too_short_bytes_is_empty_not_a_panic() {
        assert!(extension_field(&[1, 2, 3], 0).is_empty());
        assert!(extension_custom_data(&[1, 2, 3]).is_empty());
    }

    #[test]
    fn salt_hash_check_matches_the_contracts_own_binding() {
        let ext = post_interaction_only_extension(&[0xCDu8; 20]);
        let hash = U256::from_be_bytes(keccak256(&ext).0);
        let mask = (U256::from(1u8) << 160u32) - U256::from(1u8);
        let salt = hash & mask; // low 160 bits equal; high 96 bits (the real salt) can be anything
        assert!(extension_matches_salt(salt, &ext));
        assert!(!extension_matches_salt(salt + U256::from(1u8), &ext));
    }
}
