//! Builds `Erc7683AquaFiller.fill(...)` calldata from a routed plan — the on-chain settlement of an
//! ingested ERC-7683 order. Each routed leg becomes a `SourceSwap` sourcing the maker's output by
//! running its shipped Aqua order on the SwapVM router; the settler pulls that output and releases the
//! escrow inside the filler's flash callback.

use alloy::primitives::{Address, Bytes};
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};

use solvent_core::deps::ingest::{FillBuilder, FillBuilderError};
use solvent_core::primitives::ingest::Intent;
use solvent_core::primitives::registry::{Snapshot, StrategyKey};
use solvent_core::primitives::routing::{RouteLeg, RoutePlan};

use super::codec::GaslessCrossChainOrder;

sol! {
    struct Order {
        address maker;
        uint256 traits;
        bytes data;
    }
    struct SourceSwap {
        address router;
        Order order;
        address tokenIn;
        address tokenOut;
        uint256 amountOut;
        uint256 amountInMaximum;
    }
    function fill(address settler, bytes32 orderId, bytes originData, SourceSwap[] sources);
}

/// Mirrors `Erc7683AquaFiller.MAX_LEGS`. The legs nest inside one another's flash callbacks, so the
/// bound caps recursion depth and gas; a wider plan is rejected here rather than reverting on-chain.
const MAX_LEGS: usize = 4;

/// `router` is the deployment's single AquaSwapVMRouter — the `app` that keys strategies and the
/// SwapVM each source runs on.
pub struct Erc7683FillBuilder {
    router: Address,
}

impl Erc7683FillBuilder {
    pub fn new(router: Address) -> Erc7683FillBuilder {
        Erc7683FillBuilder { router }
    }
}

impl FillBuilder for Erc7683FillBuilder {
    fn build(
        &self,
        intent: &Intent,
        plan: &RoutePlan,
        snapshot: &Snapshot,
    ) -> Result<Bytes, FillBuilderError> {
        if plan.legs.is_empty() {
            return Err(FillBuilderError::NoLegs);
        }
        if plan.legs.len() > MAX_LEGS {
            return Err(FillBuilderError::TooManyLegs {
                given: plan.legs.len(),
                max: MAX_LEGS,
            });
        }
        let order = GaslessCrossChainOrder::abi_decode(&intent.raw)
            .map_err(|_| FillBuilderError::UndecodableOrder)?;

        let sources = plan
            .legs
            .iter()
            .map(|leg| self.source_for(leg, snapshot))
            .collect::<Result<Vec<_>, _>>()?;

        let call = fillCall {
            settler: intent.settler,
            orderId: intent.id.0,
            // The settler binds this to the escrow by hash, so it must be the envelope's own bytes.
            originData: order.orderData,
            sources,
        };
        Ok(Bytes::from(call.abi_encode()))
    }
}

impl Erc7683FillBuilder {
    /// One leg → the `SourceSwap` that sources its output from the maker's shipped Aqua order.
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
        let order = Order::abi_decode(&strategy.program)
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, B256, U256};

    use solvent_core::primitives::registry::AquaEvent;
    use solvent_core::primitives::{MakerId, StrategyHash};

    use super::super::builder::{OrderSpec, SignedOrderBuilder};
    use super::super::Erc7683Normalizer;
    use alloy::signers::local::PrivateKeySigner;
    use solvent_core::deps::ingest::Normalizer;

    fn router() -> Address {
        address!("9999999999999999999999999999999999999999")
    }

    fn settler() -> Address {
        address!("2222222222222222222222222222222222222222")
    }

    /// A real ingested intent, so the builder is exercised against what the normalizer actually emits.
    fn intent() -> Intent {
        let swapper = PrivateKeySigner::from_bytes(&B256::from([0x11u8; 32])).expect("test key");
        let builder = SignedOrderBuilder::new(
            address!("000000000022D473030F116dDEE9F6B43aC78BA3"),
            1,
            swapper,
        );
        let raw = builder.build(
            &OrderSpec {
                settler: settler(),
                nonce: U256::from(42u64),
                open_deadline: 1500,
                fill_deadline: 2000,
                input_token: address!("6666666666666666666666666666666666666666"),
                input_amount: U256::from(500u64),
                output_token: address!("7777777777777777777777777777777777777777"),
                output_amount: U256::from(1000u64),
                recipient: address!("8888888888888888888888888888888888888888"),
                exclusive_filler: Address::ZERO,
                exclusivity_ends: 0,
            },
            1234,
        );
        Erc7683Normalizer.normalize(&raw).expect("normalizes")
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

    fn program() -> Bytes {
        Bytes::from(
            Order {
                maker: address!("1111111111111111111111111111111111111111"),
                traits: U256::from(3u64),
                data: Bytes::from(vec![0xAB, 0xCD]),
            }
            .abi_encode(),
        )
    }

    // The calldata must carry the settler, the id the settler recorded, and the exact `orderData`
    // bytes it hashed — a mismatch on any of the three makes `fill` revert on-chain.
    #[test]
    fn calldata_targets_the_settler_with_the_escrowed_order_data() {
        let intent = intent();
        let plan = RoutePlan::new(intent.id, vec![leg()], U256::ZERO, 0.0);
        let calldata = Erc7683FillBuilder::new(router())
            .build(&intent, &plan, &snapshot_with(program()))
            .expect("builds");

        let call = fillCall::abi_decode(&calldata).expect("round-trips");
        assert_eq!(call.settler, settler());
        assert_eq!(call.orderId, intent.id.0);

        let envelope = GaslessCrossChainOrder::abi_decode(&intent.raw).expect("round-trips");
        assert_eq!(call.originData, envelope.orderData);
        assert_eq!(call.sources.len(), 1);
        assert_eq!(call.sources[0].amountOut, U256::from(1000u64));
        assert_eq!(call.sources[0].amountInMaximum, U256::from(500u64));
    }

    // The legs nest on-chain, so a plan wider than the filler's bound must never be submitted.
    #[test]
    fn too_many_legs_errors() {
        let plan = RoutePlan::new(
            intent().id,
            vec![leg(), leg(), leg(), leg(), leg()],
            U256::ZERO,
            0.0,
        );
        assert!(matches!(
            Erc7683FillBuilder::new(router()).build(&intent(), &plan, &snapshot_with(program())),
            Err(FillBuilderError::TooManyLegs { given: 5, max: 4 })
        ));
    }

    #[test]
    fn no_legs_errors() {
        let plan = RoutePlan::new(intent().id, Vec::new(), U256::ZERO, 0.0);
        assert!(matches!(
            Erc7683FillBuilder::new(router()).build(&intent(), &plan, &Snapshot::default()),
            Err(FillBuilderError::NoLegs)
        ));
    }

    #[test]
    fn missing_strategy_errors() {
        let plan = RoutePlan::new(intent().id, vec![leg()], U256::ZERO, 0.0);
        assert!(matches!(
            Erc7683FillBuilder::new(router()).build(&intent(), &plan, &Snapshot::default()),
            Err(FillBuilderError::MissingStrategy)
        ));
    }

    #[test]
    fn undecodable_order_errors() {
        let mut intent = intent();
        intent.raw = Bytes::from_static(&[1, 2, 3]);
        let plan = RoutePlan::new(intent.id, vec![leg()], U256::ZERO, 0.0);
        assert!(matches!(
            Erc7683FillBuilder::new(router()).build(&intent, &plan, &snapshot_with(program())),
            Err(FillBuilderError::UndecodableOrder)
        ));
    }
}
