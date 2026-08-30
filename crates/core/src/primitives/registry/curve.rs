//! Decoding a shipped Aqua strategy into the curve it prices as.
//!
//! A shipped `strategy` is an ABI-encoded `Order{maker, traits, data}`; the VM
//! program lives at the tail of `data` (its start is packed into `traits`). The
//! program is a sequence of `[opcode:1][argsLen:1][args:argsLen]` instructions.
//! Only the three pure-math shapes the standard Aqua strategies compile to are
//! priceable; anything with a fee, decay, gate, or unknown opcode is
//! `Unsupported` (folded for balance-correctness, never priced).

use alloy_primitives::{Bytes, U256};
use alloy_sol_types::{sol, SolValue};

sol! {
    struct Order {
        address maker;
        uint256 traits;
        bytes data;
    }
}

// AquaSwapVMRouter opcode bytes (source-array index minus one, since `_opcodes()`
// overwrites element 0 with the array length; verified against SDK-built programs).
const OP_XYC_SWAP: u8 = 0x11;
const OP_XYC_CONCENTRATE: u8 = 0x12;
const OP_PEGGED_SWAP: u8 = 0x1F;
const OP_SALT: u8 = 0x14;

const CONCENTRATE_ARGS_LEN: usize = 64;
const PEGGED_ARGS_LEN: usize = 160;

/// Pegged-curve parameters as stored in the program: `x0`/`y0` are the
/// lower/higher-address token normalization factors, `rate_lt`/`rate_gt` scale
/// each token to the common `1e18` base, `linear_width` is `A` at `1e27`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeggedParams {
    pub x0: U256,
    pub y0: U256,
    pub linear_width: U256,
    pub rate_lt: U256,
    pub rate_gt: U256,
}

/// The priceable curve an Aqua strategy decodes to, or `Unsupported`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CurveSpec {
    Xyc,
    Concentrate {
        sqrt_price_min: U256,
        sqrt_price_max: U256,
    },
    Pegged(PeggedParams),
    Unsupported,
}

/// Decode a shipped strategy (ABI-encoded `Order`) into the curve it prices as.
/// A malformed or unpriceable program yields `Unsupported`.
pub fn decode_strategy(strategy: &[u8]) -> CurveSpec {
    match program_of(strategy) {
        Some(program) => classify(&program),
        None => CurveSpec::Unsupported,
    }
}

/// The VM program: the tail of the `Order`'s `data`, starting at the program
/// slice offset Aqua packs into `traits` bits 208..224. `None` if the strategy
/// isn't a decodable `Order` or the offset is out of range.
fn program_of(strategy: &[u8]) -> Option<Bytes> {
    let order = Order::abi_decode(strategy).ok()?;
    let start = usize::try_from((order.traits >> 208) & U256::from(0xFFFFu64)).ok()?;
    (start <= order.data.len()).then(|| order.data.slice(start..))
}

/// Classify a program by its exact instruction shape (benign `salt` ignored).
fn classify(program: &[u8]) -> CurveSpec {
    let Some(instructions) = walk(program) else {
        return CurveSpec::Unsupported;
    };
    let core: Vec<(u8, &[u8])> = instructions
        .into_iter()
        .filter(|(opcode, _)| *opcode != OP_SALT)
        .collect();

    match core.as_slice() {
        [(op, _)] if *op == OP_XYC_SWAP => CurveSpec::Xyc,
        [(op_conc, args), (op_swap, _)]
            if *op_conc == OP_XYC_CONCENTRATE
                && *op_swap == OP_XYC_SWAP
                && args.len() == CONCENTRATE_ARGS_LEN =>
        {
            CurveSpec::Concentrate {
                sqrt_price_min: U256::from_be_slice(&args[0..32]),
                sqrt_price_max: U256::from_be_slice(&args[32..64]),
            }
        }
        [(op, args)] if *op == OP_PEGGED_SWAP && args.len() == PEGGED_ARGS_LEN => {
            CurveSpec::Pegged(PeggedParams {
                x0: U256::from_be_slice(&args[0..32]),
                y0: U256::from_be_slice(&args[32..64]),
                linear_width: U256::from_be_slice(&args[64..96]),
                rate_lt: U256::from_be_slice(&args[96..128]),
                rate_gt: U256::from_be_slice(&args[128..160]),
            })
        }
        _ => CurveSpec::Unsupported,
    }
}

/// Split a program into `(opcode, args)` instructions; `None` if a declared
/// `argsLen` overruns the end or a trailing opcode has no length byte.
fn walk(program: &[u8]) -> Option<Vec<(u8, &[u8])>> {
    let mut rest = program;
    let mut instructions = Vec::new();
    while let [opcode, args_len, tail @ ..] = rest {
        let (args, next) = tail.split_at_checked(usize::from(*args_len))?;
        instructions.push((*opcode, args));
        rest = next;
    }
    rest.is_empty().then_some(instructions)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A concentrate program's 64 arg bytes: two 32-byte words.
    fn conc_args(min: u8, max: u8) -> Vec<u8> {
        let mut v = vec![0u8; 64];
        v[31] = min;
        v[63] = max;
        v
    }

    #[test]
    fn classifies_the_three_curve_shapes() {
        assert_eq!(classify(&[OP_XYC_SWAP, 0]), CurveSpec::Xyc);

        let mut conc = vec![OP_XYC_CONCENTRATE, 64];
        conc.extend(conc_args(3, 9));
        conc.extend([OP_XYC_SWAP, 0]);
        assert_eq!(
            classify(&conc),
            CurveSpec::Concentrate {
                sqrt_price_min: U256::from(3u64),
                sqrt_price_max: U256::from(9u64),
            }
        );

        let mut peg = vec![OP_PEGGED_SWAP, 160];
        peg.extend(vec![0u8; 160]);
        assert!(matches!(classify(&peg), CurveSpec::Pegged(_)));
    }

    #[test]
    fn benign_salt_is_ignored() {
        assert_eq!(classify(&[OP_XYC_SWAP, 0, OP_SALT, 0]), CurveSpec::Xyc);
    }

    #[test]
    fn fee_decay_and_malformed_are_unsupported() {
        // Flat fee (0x15) and decay (0x13) prepended to a swap.
        assert_eq!(classify(&[0x15, 0, OP_XYC_SWAP, 0]), CurveSpec::Unsupported);
        assert_eq!(classify(&[0x13, 0, OP_XYC_SWAP, 0]), CurveSpec::Unsupported);
        // Unknown opcode.
        assert_eq!(classify(&[0xEE, 0]), CurveSpec::Unsupported);
        // Empty program.
        assert_eq!(classify(&[]), CurveSpec::Unsupported);
        // argsLen runs past the end.
        assert_eq!(
            classify(&[OP_XYC_CONCENTRATE, 64, 0, 0]),
            CurveSpec::Unsupported
        );
        // Concentrate with wrong arg length.
        assert_eq!(
            classify(&[OP_XYC_CONCENTRATE, 2, 0, 0, OP_XYC_SWAP, 0]),
            CurveSpec::Unsupported
        );
    }

    #[test]
    fn garbage_strategy_bytes_decode_to_unsupported() {
        assert_eq!(decode_strategy(&[]), CurveSpec::Unsupported);
        assert_eq!(decode_strategy(&[0u8; 4]), CurveSpec::Unsupported);
    }
}
