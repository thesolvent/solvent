//! Decoding a shipped Aqua strategy into the curve (plus flat fees) it prices as.
//!
//! A shipped `strategy` is an ABI-encoded `Order{maker, traits, data}`; the VM
//! program lives at the tail of `data` (its start is packed into `traits`). The
//! program is a sequence of `[opcode:1][argsLen:1][args:argsLen]` instructions:
//! one of the three curve shapes, optionally wrapped in flat fees (priced) and
//! guards (price-neutral, skipped). Anything else — decay, jumps, protocol/
//! dynamic fees, extruction, unknown — is `Unsupported` (folded for
//! balance-correctness, never priced).

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
const OP_FLAT_FEE: u8 = 0x15;

/// Opcodes that don't change the quote amount, so pricing skips them: `salt` and
/// the deadline + taker/`tx.origin` balance guards. (Static protocol fees also
/// reduce the quote but round down, unlike `flatFee`'s round-up — deferred.)
const PRICE_NEUTRAL: [u8; 6] = [OP_SALT, 0x0D, 0x0E, 0x0F, 0x10, 0x21];

const CONCENTRATE_ARGS_LEN: usize = 64;
const PEGGED_ARGS_LEN: usize = 160;
const FEE_BPS_DENOMINATOR: u64 = 1_000_000_000; // SwapVM `BPS`: 1e9 = 100%

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

/// A priceable swap curve (the shape only — fees are tracked separately).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Curve {
    Xyc,
    Concentrate {
        sqrt_price_min: U256,
        sqrt_price_max: U256,
    },
    Pegged(PeggedParams),
}

/// What an Aqua strategy decodes to: a priceable curve plus its ordered flat
/// fees (input-side, `bps` at `1e9`), or `Unsupported`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CurveSpec {
    Priceable { curve: Curve, fees_in_bps: Vec<u32> },
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

/// Partition a program into price-neutral opcodes (skipped), flat fees (`bps`
/// collected in program order), and the curve shape. Anything else (jump, decay,
/// dynamic fee, extruction, unknown) → `Unsupported`.
fn classify(program: &[u8]) -> CurveSpec {
    let Some(instructions) = walk(program) else {
        return CurveSpec::Unsupported;
    };
    let mut fees_in_bps = Vec::new();
    let mut curve_ixs: Vec<(u8, &[u8])> = Vec::new();
    for (opcode, args) in instructions {
        match opcode {
            OP_FLAT_FEE => match parse_flat_fee(args) {
                Some(bps) => fees_in_bps.push(bps),
                None => return CurveSpec::Unsupported,
            },
            OP_XYC_SWAP | OP_XYC_CONCENTRATE | OP_PEGGED_SWAP => curve_ixs.push((opcode, args)),
            _ if PRICE_NEUTRAL.contains(&opcode) => {}
            _ => return CurveSpec::Unsupported,
        }
    }
    match curve_of(&curve_ixs) {
        Some(curve) => CurveSpec::Priceable { curve, fees_in_bps },
        None => CurveSpec::Unsupported,
    }
}

/// The curve shape from just the swap-defining opcodes.
fn curve_of(curve_ixs: &[(u8, &[u8])]) -> Option<Curve> {
    match curve_ixs {
        [(op, _)] if *op == OP_XYC_SWAP => Some(Curve::Xyc),
        [(op_conc, args), (op_swap, _)]
            if *op_conc == OP_XYC_CONCENTRATE
                && *op_swap == OP_XYC_SWAP
                && args.len() == CONCENTRATE_ARGS_LEN =>
        {
            Some(Curve::Concentrate {
                sqrt_price_min: U256::from_be_slice(&args[0..32]),
                sqrt_price_max: U256::from_be_slice(&args[32..64]),
            })
        }
        [(op, args)] if *op == OP_PEGGED_SWAP && args.len() == PEGGED_ARGS_LEN => {
            Some(Curve::Pegged(PeggedParams {
                x0: U256::from_be_slice(&args[0..32]),
                y0: U256::from_be_slice(&args[32..64]),
                linear_width: U256::from_be_slice(&args[64..96]),
                rate_lt: U256::from_be_slice(&args[96..128]),
                rate_gt: U256::from_be_slice(&args[128..160]),
            }))
        }
        _ => None,
    }
}

/// `flatFee` args: a 4-byte `uint32` bps (≤ `1e9`).
fn parse_flat_fee(args: &[u8]) -> Option<u32> {
    let bytes: [u8; 4] = args.try_into().ok()?;
    let bps = u32::from_be_bytes(bytes);
    (u64::from(bps) <= FEE_BPS_DENOMINATOR).then_some(bps)
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

    fn conc_args(min: u8, max: u8) -> Vec<u8> {
        let mut v = vec![0u8; 64];
        v[31] = min;
        v[63] = max;
        v
    }
    fn flat_fee(bps: u32) -> Vec<u8> {
        let mut v = vec![OP_FLAT_FEE, 4];
        v.extend(bps.to_be_bytes());
        v
    }
    fn priceable(curve: Curve, fees: &[u32]) -> CurveSpec {
        CurveSpec::Priceable {
            curve,
            fees_in_bps: fees.to_vec(),
        }
    }

    #[test]
    fn classifies_the_three_curve_shapes() {
        assert_eq!(classify(&[OP_XYC_SWAP, 0]), priceable(Curve::Xyc, &[]));

        let mut conc = vec![OP_XYC_CONCENTRATE, 64];
        conc.extend(conc_args(3, 9));
        conc.extend([OP_XYC_SWAP, 0]);
        assert_eq!(
            classify(&conc),
            priceable(
                Curve::Concentrate {
                    sqrt_price_min: U256::from(3u64),
                    sqrt_price_max: U256::from(9u64),
                },
                &[]
            )
        );

        let mut peg = vec![OP_PEGGED_SWAP, 160];
        peg.extend(vec![0u8; 160]);
        assert!(matches!(
            classify(&peg),
            CurveSpec::Priceable {
                curve: Curve::Pegged(_),
                ..
            }
        ));
    }

    #[test]
    fn flat_fees_are_extracted_in_program_order() {
        let mut p = flat_fee(30);
        p.extend([OP_XYC_SWAP, 0]);
        assert_eq!(classify(&p), priceable(Curve::Xyc, &[30]));

        let mut stacked = flat_fee(10);
        stacked.extend(flat_fee(20));
        stacked.extend([OP_XYC_SWAP, 0]);
        assert_eq!(classify(&stacked), priceable(Curve::Xyc, &[10, 20]));
    }

    #[test]
    fn price_neutral_opcodes_are_ignored() {
        // salt + a taker gate (1-byte arg) leave the price untouched.
        let program = [OP_SALT, 0, 0x0E, 1, 0xAB, OP_XYC_SWAP, 0];
        assert_eq!(classify(&program), priceable(Curve::Xyc, &[]));
    }

    #[test]
    fn non_pure_and_malformed_are_unsupported() {
        // decay, jump, protocol fees (floor-rounded, deferred), dynamic fee,
        // extruction, unknown.
        for op in [0x13u8, 0x0A, 0x1B, 0x1C, 0x1D, 0x20, 0xEE] {
            assert_eq!(classify(&[op, 0, OP_XYC_SWAP, 0]), CurveSpec::Unsupported);
        }
        assert_eq!(classify(&[]), CurveSpec::Unsupported);
        assert_eq!(
            classify(&[OP_XYC_CONCENTRATE, 64, 0, 0]),
            CurveSpec::Unsupported
        );
        assert_eq!(
            classify(&[OP_XYC_CONCENTRATE, 2, 0, 0, OP_XYC_SWAP, 0]),
            CurveSpec::Unsupported
        );
        // flatFee with a bad (non-4-byte) arg.
        assert_eq!(
            classify(&[OP_FLAT_FEE, 0, OP_XYC_SWAP, 0]),
            CurveSpec::Unsupported
        );
    }

    #[test]
    fn garbage_strategy_bytes_decode_to_unsupported() {
        assert_eq!(decode_strategy(&[]), CurveSpec::Unsupported);
        assert_eq!(decode_strategy(&[0u8; 4]), CurveSpec::Unsupported);
    }
}
