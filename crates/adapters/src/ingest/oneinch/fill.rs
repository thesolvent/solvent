//! Builds `OneInchLimitOrderAquaFiller.fill(...)` calldata from a routed plan. Each routed leg
//! becomes a `SourceSwap` that sources the order's taker-asset leg by running the maker's shipped
//! Aqua order on the SwapVM router — the same sourcing mechanism `UniswapXFillBuilder` uses, behind
//! a different on-chain filler contract because 1inch's protocol calls the taker back mid-fill
//! rather than the reactor-callback shape UniswapX uses.

use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};

use solvent_core::deps::ingest::{BuiltFill, FillBuilder, FillBuilderError};
use solvent_core::primitives::ingest::Intent;
use solvent_core::primitives::registry::{Snapshot, StrategyKey};
use solvent_core::primitives::routing::{RouteLeg, RoutePlan};

/// `_MAKER_AMOUNT_FLAG` (bit 255 of `TakerTraits`, per `TakerTraitsLib.sol`): `amount` below is a
/// making amount, and the protocol computes the taking amount from it. This codebase never submits
/// a partial fill (`OneInchNormalizer` only ever quotes an order's full stated amounts), so
/// `amount` is always the order's full `makingAmount`.
fn maker_amount_flag() -> U256 {
    U256::from(1u8) << 255
}

sol! {
    // Same wire shape as `codec::Order`/`WireOrder` — `sol!` binds a function's struct parameters
    // within its own invocation, so this is redeclared here rather than imported
    // (`UniswapXFillBuilder` does the same for its own order type). The encoded *bytes* for
    // `maker`/`receiver`/`makerAsset`/`takerAsset` are identical whether declared `address` or
    // `uint256` (both are 32-byte words), but the function *selector* is not: 1inch's real
    // `IOrderMixin.Order` types these fields as its own `Address`/`MakerTraits` custom value
    // types, which the Solidity compiler canonicalizes to their underlying `uint256` in a
    // function selector (confirmed against the compiled `OneInchLimitOrderAquaFiller` ABI:
    // `fill(...)` selector `55ce07d9` takes an all-`uint256` order tuple). Declaring these as
    // `address` here would compute a different, wrong selector — the real contract has no such
    // function, so the call reverts.
    struct Order {
        uint256 salt;
        uint256 maker;
        uint256 receiver;
        uint256 makerAsset;
        uint256 takerAsset;
        uint256 makingAmount;
        uint256 takingAmount;
        uint256 makerTraits;
    }
    struct WireOrder {
        Order order;
        bytes extension;
    }
    struct SwapVmOrder {
        address maker;
        uint256 traits;
        bytes data;
    }
    struct SourceSwap {
        address router;
        SwapVmOrder order;
        address tokenIn;
        address tokenOut;
        uint256 amountOut;
        uint256 amountInMaximum;
    }
    function fill(
        Order order,
        bytes32 r,
        bytes32 vs,
        uint256 amount,
        uint256 takerTraitsBase,
        bytes extension,
        SourceSwap[] sources
    );
}

/// `router` is the deployment's single AquaSwapVMRouter, identical to `UniswapXFillBuilder`'s.
/// `filler` is this builder's own deployed `OneInchLimitOrderAquaFiller` — the contract a built
/// fill must be sent to.
pub struct OneInchFillBuilder {
    router: Address,
    filler: Address,
}

impl OneInchFillBuilder {
    pub fn new(router: Address, filler: Address) -> OneInchFillBuilder {
        OneInchFillBuilder { router, filler }
    }
}

impl FillBuilder for OneInchFillBuilder {
    fn build(
        &self,
        intent: &Intent,
        plan: &RoutePlan,
        snapshot: &Snapshot,
    ) -> Result<BuiltFill, FillBuilderError> {
        if plan.legs.is_empty() {
            return Err(FillBuilderError::NoLegs);
        }
        let wire = WireOrder::abi_decode(&intent.raw)
            .map_err(|_| FillBuilderError::MalformedIntent("1inch order"))?;
        let (r, vs) = split_signature(&intent.signature)?;
        let amount = wire.order.makingAmount;
        let sources = plan
            .legs
            .iter()
            .map(|leg| self.source_for(leg, snapshot))
            .collect::<Result<Vec<_>, _>>()?;

        let call = fillCall {
            order: wire.order,
            r,
            vs,
            amount,
            takerTraitsBase: maker_amount_flag(),
            extension: wire.extension,
            sources,
        };
        Ok(BuiltFill::new(self.filler, Bytes::from(call.abi_encode())))
    }
}

impl OneInchFillBuilder {
    /// One leg → the `SourceSwap` that sources its output from the maker's shipped Aqua order.
    /// Identical in shape to `UniswapXFillBuilder::source_for` — a different `sol!`-bound
    /// `SourceSwap` type, since each filler's ABI is declared independently.
    fn source_for(
        &self,
        leg: &RouteLeg,
        snapshot: &Snapshot,
    ) -> Result<SourceSwap, FillBuilderError> {
        let key = StrategyKey {
            maker: leg.maker,
            app: self.router,
            strategy_hash: leg.strategy_hash,
        };
        let strategy = snapshot
            .strategy(&key)
            .ok_or(FillBuilderError::MissingStrategy)?;
        let order = SwapVmOrder::abi_decode(&strategy.program)
            .map_err(|_| FillBuilderError::UndecodableProgram)?;
        Ok(SourceSwap {
            router: self.router,
            order,
            tokenIn: leg.token_in,
            tokenOut: leg.token_out,
            amountOut: leg.amount_out,
            amountInMaximum: leg.amount_in,
        })
    }
}

/// A 65-byte `r || s || v` ECDSA signature (as 1inch's orderbook API returns it) into the
/// protocol's compact `(r, vs)` pair (EIP-2098: `v`'s recovery bit — 0 for `v == 27`, 1 for
/// `v == 28` — packed into `s`'s otherwise-unused top bit).
fn split_signature(sig: &[u8]) -> Result<(B256, B256), FillBuilderError> {
    if sig.len() != 65 {
        return Err(FillBuilderError::MalformedIntent("signature"));
    }
    let mut s = [0u8; 32];
    s.copy_from_slice(&sig[32..64]);
    if sig[64] & 1 == 0 {
        s[0] |= 0x80;
    }
    Ok((B256::from_slice(&sig[0..32]), B256::from(s)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, U256};

    use solvent_core::primitives::registry::AquaEvent;
    use solvent_core::primitives::{MakerId, StrategyHash};

    fn router() -> Address {
        address!("9999999999999999999999999999999999999999")
    }

    fn filler() -> Address {
        address!("8888888888888888888888888888888888888888")
    }

    fn leg() -> RouteLeg {
        RouteLeg {
            maker: MakerId(Address::from([1u8; 20])),
            strategy_hash: StrategyHash(B256::from([7u8; 32])),
            token_in: address!("6666666666666666666666666666666666666666"),
            token_out: address!("7777777777777777777777777777777777777777"),
            amount_in: U256::from(500u64),
            amount_out: U256::from(1000u64),
        }
    }

    fn snapshot_with(program: Bytes) -> Snapshot {
        let l = leg();
        let mut snap = Snapshot::default();
        snap.apply(AquaEvent::Shipped {
            maker: l.maker,
            app: router(),
            strategy_hash: l.strategy_hash,
            strategy: program,
        });
        snap
    }

    #[test]
    fn source_maps_a_leg_and_recovers_the_order() {
        let order = SwapVmOrder {
            maker: address!("1111111111111111111111111111111111111111"),
            traits: U256::from(3u64),
            data: Bytes::from(vec![0xAB, 0xCD]),
        };
        let snap = snapshot_with(Bytes::from(order.abi_encode()));

        let l = leg();
        let src = OneInchFillBuilder::new(router(), filler())
            .source_for(&l, &snap)
            .expect("maps");
        assert_eq!(src.router, router());
        assert_eq!(src.order.maker, order.maker);
        assert_eq!(src.tokenIn, l.token_in);
        assert_eq!(src.tokenOut, l.token_out);
        assert_eq!(src.amountOut, l.amount_out);
        assert_eq!(src.amountInMaximum, l.amount_in);
    }

    #[test]
    fn missing_strategy_errors() {
        let r =
            OneInchFillBuilder::new(router(), filler()).source_for(&leg(), &Snapshot::default());
        assert!(matches!(r, Err(FillBuilderError::MissingStrategy)));
    }

    #[test]
    fn split_signature_packs_odd_v_into_s_top_bit() {
        let mut sig = vec![0xAAu8; 32]; // r
        sig.extend(vec![0x11u8; 32]); // s (top bit clear)
        sig.push(28); // v (odd recovery bit)

        let (r, vs) = split_signature(&sig).expect("splits");
        assert_eq!(r, B256::from_slice(&[0xAAu8; 32]));
        assert_eq!(vs.0[0], 0x91, "top bit of s set for v=28");
    }

    #[test]
    fn split_signature_leaves_s_top_bit_clear_for_even_v() {
        let mut sig = vec![0xAAu8; 32];
        sig.extend(vec![0x11u8; 32]);
        sig.push(27);

        let (_, vs) = split_signature(&sig).expect("splits");
        assert_eq!(vs.0[0], 0x11, "top bit of s clear for v=27");
    }

    #[test]
    fn split_signature_rejects_the_wrong_length() {
        assert!(matches!(
            split_signature(&[0u8; 64]),
            Err(FillBuilderError::MalformedIntent("signature"))
        ));
    }
}
