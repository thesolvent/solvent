//! Builds `UniswapXAquaFiller.fill(...)` calldata from a routed plan — the on-chain settlement of an
//! ingested UniswapX order. Each routed leg becomes a `SourceSwap` that sources the maker's output
//! by running its shipped Aqua order on the SwapVM router; the signed order forwards verbatim.

use alloy::primitives::{Address, Bytes};
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};

use solvent_core::deps::ingest::{FillBuilder, FillBuilderError};
use solvent_core::primitives::ingest::Intent;
use solvent_core::primitives::registry::{Snapshot, StrategyKey};
use solvent_core::primitives::routing::{RouteLeg, RoutePlan};

sol! {
    struct Order {
        address maker;
        uint256 traits;
        bytes data;
    }
    struct SignedOrder {
        bytes order;
        bytes sig;
    }
    struct SourceSwap {
        address router;
        Order order;
        address tokenIn;
        address tokenOut;
        uint256 amountOut;
        uint256 amountInMaximum;
    }
    function fill(address reactor, SignedOrder order, SourceSwap[] sources);
}

/// `router` is the deployment's single AquaSwapVMRouter — the `app` that keys strategies and the
/// SwapVM each source runs on.
pub struct UniswapXFillBuilder {
    router: Address,
}

impl UniswapXFillBuilder {
    pub fn new(router: Address) -> UniswapXFillBuilder {
        UniswapXFillBuilder { router }
    }
}

impl FillBuilder for UniswapXFillBuilder {
    fn build(
        &self,
        intent: &Intent,
        plan: &RoutePlan,
        snapshot: &Snapshot,
    ) -> Result<Bytes, FillBuilderError> {
        if plan.legs.is_empty() {
            return Err(FillBuilderError::NoLegs);
        }
        let sources = plan
            .legs
            .iter()
            .map(|leg| self.source_for(leg, snapshot))
            .collect::<Result<Vec<_>, _>>()?;

        let call = fillCall {
            reactor: intent.settler,
            order: SignedOrder {
                order: intent.raw.clone(),
                sig: intent.signature.clone(),
            },
            sources,
        };
        Ok(Bytes::from(call.abi_encode()))
    }
}

impl UniswapXFillBuilder {
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

    fn router() -> Address {
        address!("9999999999999999999999999999999999999999")
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
        let order = Order {
            maker: address!("1111111111111111111111111111111111111111"),
            traits: U256::from(3u64),
            data: Bytes::from(vec![0xAB, 0xCD]),
        };
        let snap = snapshot_with(Bytes::from(order.abi_encode()));

        let l = leg();
        let src = UniswapXFillBuilder::new(router())
            .source_for(&l, &snap)
            .expect("maps");
        assert_eq!(src.router, router());
        assert_eq!(src.order.maker, order.maker);
        assert_eq!(src.order.traits, order.traits);
        assert_eq!(src.order.data, order.data);
        assert_eq!(src.tokenIn, l.token_in);
        assert_eq!(src.tokenOut, l.token_out);
        assert_eq!(src.amountOut, l.amount_out);
        assert_eq!(src.amountInMaximum, l.amount_in);
    }

    #[test]
    fn missing_strategy_errors() {
        let r = UniswapXFillBuilder::new(router()).source_for(&leg(), &Snapshot::default());
        assert!(matches!(r, Err(FillBuilderError::MissingStrategy)));
    }

    #[test]
    fn undecodable_program_errors() {
        let snap = snapshot_with(Bytes::from(vec![1, 2, 3]));
        let r = UniswapXFillBuilder::new(router()).source_for(&leg(), &snap);
        assert!(matches!(r, Err(FillBuilderError::UndecodableProgram)));
    }
}
