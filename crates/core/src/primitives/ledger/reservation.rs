//! A reservation: the promise to pull maker capital across one or more strategies, its owning
//! workflow, and where that promise sits in the two-phase lifecycle.

use alloy_primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};

use crate::primitives::{IntentId, MakerId, RebateBatchId, ReservationId, StrategyHash};

use super::account::AccountKey;

/// One maker-capital source a reservation draws on: `amount` of `token` pulled from a specific
/// strategy — settled by exactly one Aqua `pull`. Each source holds capacity at both ceilings, the
/// shared wallet and the strategy virtual.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ReservationSource {
    #[schema(value_type = String)]
    pub maker: MakerId,
    #[schema(value_type = String)]
    pub strategy_hash: StrategyHash,
    #[schema(value_type = String)]
    pub token: Address,
    #[schema(value_type = String)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReservationState {
    Pending,
    Committed,
    Posted,
    Voided,
    Expired,
    ReorgOpen,
}

/// The operation whose inventory promise this reservation protects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReservationOwner {
    Swap(IntentId),
    Rebate(RebateBatchId),
}

impl ReservationOwner {
    pub fn kind(self) -> &'static str {
        match self {
            Self::Swap(_) => "swap",
            Self::Rebate(_) => "rebate",
        }
    }

    pub fn id(self) -> B256 {
        match self {
            Self::Swap(intent) => intent.0,
            Self::Rebate(batch) => batch.0,
        }
    }

    /// The swap intent when this is a normal fill reservation.
    pub fn swap_intent(self) -> Option<IntentId> {
        match self {
            Self::Swap(intent) => Some(intent),
            Self::Rebate(_) => None,
        }
    }
}

/// A promise to pull maker capital across one or more `sources`. Created `Pending`; the ledger owns
/// every subsequent transition.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Reservation {
    pub id: ReservationId,
    pub owner: ReservationOwner,
    pub sources: Vec<ReservationSource>,
    pub state: ReservationState,
    /// Unix seconds after which a TTL sweep may expire the reservation.
    pub expires_at: u64,
}

impl Reservation {
    /// A fresh `Pending` reservation. Only the ledger moves it out of `Pending`.
    pub fn new(
        id: ReservationId,
        owner: ReservationOwner,
        sources: Vec<ReservationSource>,
        expires_at: u64,
    ) -> Self {
        Self {
            id,
            owner,
            sources,
            state: ReservationState::Pending,
            expires_at,
        }
    }

    /// A fresh reservation owned by a normal swap intent.
    pub fn for_swap(
        id: ReservationId,
        intent: IntentId,
        sources: Vec<ReservationSource>,
        expires_at: u64,
    ) -> Self {
        Self::new(id, ReservationOwner::Swap(intent), sources, expires_at)
    }

    /// A fresh reservation owned by a price-restoration batch.
    pub fn for_rebate(
        id: ReservationId,
        batch: RebateBatchId,
        sources: Vec<ReservationSource>,
        expires_at: u64,
    ) -> Self {
        Self::new(id, ReservationOwner::Rebate(batch), sources, expires_at)
    }
}
