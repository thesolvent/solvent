//! The two ceilings a maker pull must satisfy — the shared wallet and the strategy virtual — as
//! ledger accounts.

use alloy_primitives::Address;

use crate::primitives::{MakerId, StrategyHash};

/// A ceiling a reserved pull is held against: `WalletBudget` (shared, `min(balanceOf, allowance)`) or
/// `StrategyVirtual` (one strategy's balance). A source holds at both, so the shared wallet binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AccountKey {
    WalletBudget {
        maker: MakerId,
        token: Address,
    },
    StrategyVirtual {
        maker: MakerId,
        strategy_hash: StrategyHash,
        token: Address,
    },
}
