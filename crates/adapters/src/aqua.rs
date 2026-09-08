//! The Aqua liquidity-layer event ABI and its decode into domain [`AquaEvent`]s — shared by the
//! registry chain-source (ingest), the execution settlement reader (fill accounting), and the
//! recapture settled-legs reader, the places that decode Aqua logs.

use std::sync::Arc;

use alloy::primitives::{Address, B256};
use alloy::sol;
use solvent_core::primitives::{registry::AquaEvent, MakerId, StrategyHash};

use crate::events::prelude::{process_logs, Provider};

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

/// Decode every Aqua event that `aqua` emitted in `tx`'s receipt — the shared read behind the
/// settlement reader (per-source pulls) and the recapture settled-legs reader (per-leg pushes+pulls).
pub(crate) async fn events_in_tx(
    provider: &Arc<dyn Provider>,
    aqua: Address,
    tx: B256,
) -> Result<Vec<AquaEvent>, String> {
    let receipt = provider
        .get_transaction_receipt(tx)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no receipt for tx {tx}"))?;
    Ok(process_logs::<IAqua::IAquaEvents>(receipt.logs(), &[aqua])
        .into_iter()
        .map(|ext| into_domain(ext.event))
        .collect())
}
