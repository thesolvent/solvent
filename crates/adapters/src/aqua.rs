//! The Aqua liquidity-layer event ABI and its decode into domain [`AquaEvent`]s — shared by the
//! registry chain-source (ingest) and the execution settlement reader (fill accounting), the two
//! places that decode Aqua logs.

use alloy::sol;
use solvent_core::primitives::{registry::AquaEvent, MakerId, StrategyHash};

sol! {
    interface IAqua {
        event Shipped(address maker, address app, bytes32 strategyHash, bytes strategy);
        event Pushed(address maker, address app, bytes32 strategyHash, address token, uint256 amount);
        event Pulled(address maker, address app, bytes32 strategyHash, address token, uint256 amount);
        event Docked(address maker, address app, bytes32 strategyHash);
    }
}

/// Lift a decoded chain-ABI event into its domain form.
pub(crate) fn into_domain(event: IAqua::IAquaEvents) -> AquaEvent {
    match event {
        IAqua::IAquaEvents::Shipped(e) => AquaEvent::Shipped {
            maker: MakerId(e.maker),
            app: e.app,
            strategy_hash: StrategyHash(e.strategyHash),
            strategy: e.strategy,
        },
        IAqua::IAquaEvents::Pushed(e) => AquaEvent::Pushed {
            maker: MakerId(e.maker),
            app: e.app,
            strategy_hash: StrategyHash(e.strategyHash),
            token: e.token,
            amount: e.amount,
        },
        IAqua::IAquaEvents::Pulled(e) => AquaEvent::Pulled {
            maker: MakerId(e.maker),
            app: e.app,
            strategy_hash: StrategyHash(e.strategyHash),
            token: e.token,
            amount: e.amount,
        },
        IAqua::IAquaEvents::Docked(e) => AquaEvent::Docked {
            maker: MakerId(e.maker),
            app: e.app,
            strategy_hash: StrategyHash(e.strategyHash),
        },
    }
}
