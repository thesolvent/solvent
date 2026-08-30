//! The four Aqua liquidity-layer events, mirrored from `IAqua`.
//!
//! `ship()` emits [`AquaEvent::Shipped`] (registering the strategy program) and
//! then one [`AquaEvent::Pushed`] per token for the initial balances — so the
//! whole balance picture arrives through `Pushed`/`Pulled`, and the fold never
//! needs the shipped amounts from `Shipped` itself. A swap settles as a `Pushed`
//! (taker → maker, `tokenIn`) plus a `Pulled` (maker → taker, `tokenOut`);
//! `dock()` closes the strategy.

use alloy_primitives::{Address, Bytes, U256};

use crate::primitives::{MakerId, StrategyHash};

/// A decoded Aqua event. `app` is the SwapVM router the strategy runs on; there
/// is no typed id for it (one router per deployment in the MVP).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AquaEvent {
    /// A maker registered a strategy. `strategy` is the raw shipped bytes
    /// (an ABI-encoded `Order`); the Tier-0 curve is decoded from it later.
    Shipped {
        maker: MakerId,
        app: Address,
        strategy_hash: StrategyHash,
        strategy: Bytes,
    },
    /// `token` balance increased by `amount` (initial ship leg, or a swap inflow).
    Pushed {
        maker: MakerId,
        app: Address,
        strategy_hash: StrategyHash,
        token: Address,
        amount: U256,
    },
    /// `token` balance decreased by `amount` (a swap outflow at settlement).
    Pulled {
        maker: MakerId,
        app: Address,
        strategy_hash: StrategyHash,
        token: Address,
        amount: U256,
    },
    /// The maker closed the strategy; all its token balances are tombstoned.
    Docked {
        maker: MakerId,
        app: Address,
        strategy_hash: StrategyHash,
    },
}

impl AquaEvent {
    /// The `(maker, app, strategy_hash)` this event addresses.
    pub fn key(&self) -> StrategyKey {
        match *self {
            AquaEvent::Shipped {
                maker,
                app,
                strategy_hash,
                ..
            }
            | AquaEvent::Pushed {
                maker,
                app,
                strategy_hash,
                ..
            }
            | AquaEvent::Pulled {
                maker,
                app,
                strategy_hash,
                ..
            }
            | AquaEvent::Docked {
                maker,
                app,
                strategy_hash,
            } => StrategyKey {
                maker,
                app,
                strategy_hash,
            },
        }
    }
}

/// The identity of one shipped strategy: the exact triple Aqua keys balances by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StrategyKey {
    pub maker: MakerId,
    pub app: Address,
    pub strategy_hash: StrategyHash,
}

/// The chain-global position of an event in the canonical log. `Ord` compares
/// `block_number` then `log_index`, so it increases monotonically along the
/// stream — letting the fold enforce exactly-once + ordering. Reorg detection
/// (a changed block hash) lives at the store/sync layer, not here; a single
/// chain in the MVP, so no `chain_id` field yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventCursor {
    pub block_number: u64,
    pub log_index: u64,
}
