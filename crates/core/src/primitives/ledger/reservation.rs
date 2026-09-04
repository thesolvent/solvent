//! A reservation: the promise to pull maker capital across one or more strategies to fill an
//! intent, and where that promise sits in the two-phase lifecycle.

use alloy_primitives::{Address, U256};

use crate::primitives::{IntentId, MakerId, ReservationId, StrategyHash};

use super::account::AccountKey;

/// One maker-capital source a reservation draws on: `amount` of `token` pulled from a specific
/// strategy — settled by exactly one Aqua `pull`. Each source holds capacity at both ceilings, the
/// shared wallet and the strategy virtual.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationSource {
    pub maker: MakerId,
    pub strategy_hash: StrategyHash,
    pub token: Address,
    pub amount: U256,
}

impl ReservationSource {
    /// The two accounts this source is held against.
    pub fn accounts(&self) -> [AccountKey; 2] {
        [
            AccountKey::WalletBudget {
                maker: self.maker,
                token: self.token,
            },
            AccountKey::StrategyVirtual {
                maker: self.maker,
                strategy_hash: self.strategy_hash,
                token: self.token,
            },
        ]
    }
}

/// The two-phase lifecycle of a reservation. `Pending` holds at both ceilings; a terminal state
/// releases them (`Voided`/`Expired`) or consumes them (`Posted`). `ReorgOpen` is a posted
/// settlement that a reorg rolled back — its consumption is reversed and reconcile re-decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReservationState {
    Pending,
    Posted,
    Voided,
    Expired,
    ReorgOpen,
}

/// A promise to fill an intent by pulling maker capital across one or more `sources`. Created
/// `Pending`; the ledger owns every subsequent transition.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Reservation {
    pub id: ReservationId,
    pub intent: IntentId,
    pub sources: Vec<ReservationSource>,
    pub state: ReservationState,
    /// Unix seconds after which a TTL sweep may expire the reservation.
    pub expires_at: u64,
}

impl Reservation {
    /// A fresh `Pending` reservation. Only the ledger moves it out of `Pending`.
    pub fn new(
        id: ReservationId,
        intent: IntentId,
        sources: Vec<ReservationSource>,
        expires_at: u64,
    ) -> Self {
        Self {
            id,
            intent,
            sources,
            state: ReservationState::Pending,
            expires_at,
        }
    }
}
