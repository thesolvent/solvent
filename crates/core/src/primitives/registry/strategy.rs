//! A tracked maker strategy and the unordered token pair it trades.

use std::collections::BTreeMap;

use alloy_primitives::{Address, Bytes, U256};

use super::curve::{decode_strategy, CurveSpec};
use super::event::StrategyKey;

/// An unordered token pair, canonicalized so `(A,B)` and `(B,A)` are one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TokenPair {
    pub lo: Address,
    pub hi: Address,
}

impl TokenPair {
    /// Order the two tokens by address (the on-chain `Lt`/`Gt` convention).
    pub fn new(a: Address, b: Address) -> Self {
        Self {
            lo: a.min(b),
            hi: a.max(b),
        }
    }
}

/// One maker strategy's live state, folded from the Aqua event stream. Balances
/// are the maker's virtual allowances Aqua tracks; `curve` is decoded once from
/// the shipped program. Docked strategies are kept as inactive tombstones so
/// replay is deterministic and re-ship stays a no-op.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MakerStrategy {
    pub key: StrategyKey,
    pub curve: CurveSpec,
    /// token -> virtual balance.
    pub balances: BTreeMap<Address, U256>,
    /// False once `Docked`.
    pub active: bool,
    /// The shipped Aqua `Order`, verbatim — the fill builder needs it to source from this strategy.
    pub program: Bytes,
}

impl MakerStrategy {
    /// Register a strategy, decoding its curve from the shipped bytes.
    pub fn new(key: StrategyKey, strategy: &[u8]) -> Self {
        Self {
            key,
            curve: decode_strategy(strategy),
            balances: BTreeMap::new(),
            active: true,
            program: Bytes::copy_from_slice(strategy),
        }
    }

    /// Virtual balance of `token` (zero if the strategy holds none).
    pub fn balance(&self, token: &Address) -> U256 {
        self.balances.get(token).copied().unwrap_or(U256::ZERO)
    }

    /// The traded pair iff exactly two tokens are held — the MVP strategy shape.
    /// Strategies with any other token count are not pair-routable.
    pub fn pair(&self) -> Option<TokenPair> {
        let mut tokens = self.balances.keys();
        match (tokens.next(), tokens.next(), tokens.next()) {
            (Some(&a), Some(&b), None) => Some(TokenPair::new(a, b)),
            _ => None,
        }
    }
}
