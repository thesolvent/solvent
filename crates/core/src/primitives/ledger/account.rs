//! The two ledger accounts a maker pull must satisfy. A pull reverts on-chain unless it fits
//! within both the maker's shared wallet allowance and the individual strategy's virtual balance,
//! so off-chain we place a hold at each — one account per ceiling.

use alloy_primitives::Address;

use crate::primitives::{MakerId, StrategyHash};

/// A ceiling a reserved pull is held against. `WalletBudget` is shared across all of a maker's
/// strategies and apps (`min(balanceOf, allowance→Aqua)`); `StrategyVirtual` is one strategy's
/// own virtual balance of a token. A source holds at both, so the shared wallet binds even when two
/// strategies each look individually fundable.
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
