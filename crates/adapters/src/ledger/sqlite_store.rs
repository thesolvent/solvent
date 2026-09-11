//! The SQLite ledger store (sqlx) behind the `LedgerStore` port. Each command is one statement;
//! `reserve` is idempotent on the id and transitions fire only from the expected prior state.

use alloy::primitives::{B256, U256};
use async_trait::async_trait;
use solvent_core::{
    deps::ledger::{LedgerStore, LedgerStoreError},
    primitives::{
        ledger::{Reservation, ReservationOwner, ReservationSource},
        IntentId, RebateBatchId, ReservationId,
    },
};
use sqlx::SqlitePool;

pub struct SqliteLedgerStore {
    pool: SqlitePool,
}

impl SqliteLedgerStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Apply the embedded schema migrations.
    pub async fn migrate(&self) -> Result<(), LedgerStoreError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(db)
    }

    /// Move a reservation from `from` to `to`; a no-op unless it is currently in `from`, which makes
    /// a replayed transition idempotent. Both labels are code constants, never caller input.
    async fn transition(
        &self,
        id: ReservationId,
        to: &str,
        from: &str,
    ) -> Result<(), LedgerStoreError> {
        sqlx::query(&format!(
            "UPDATE ledger_reservation SET state = '{to}' WHERE id = ? AND state = '{from}'"
        ))
        .bind(id.0.to_vec())
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }
}

fn db(e: impl std::fmt::Display) -> LedgerStoreError {
    LedgerStoreError::Db(e.to_string())
}

fn i64_of(value: u64) -> Result<i64, LedgerStoreError> {
    i64::try_from(value).map_err(|_| LedgerStoreError::Db(format!("value {value} exceeds i64")))
}

fn b256(bytes: &[u8]) -> Result<B256, LedgerStoreError> {
    B256::try_from(bytes)
        .map_err(|_| LedgerStoreError::Db(format!("expected a 32-byte id, got {}", bytes.len())))
}

#[async_trait]
impl LedgerStore for SqliteLedgerStore {
    async fn reserve(&self, reservation: &Reservation) -> Result<(), LedgerStoreError> {
        let sources = serde_json::to_string(&reservation.sources).map_err(db)?;
        let owner_kind = reservation.owner.kind();
        let owner_id = reservation.owner.id();
        let mut transaction = self.pool.begin().await.map_err(db)?;
        let inserted = sqlx::query(
            "INSERT INTO ledger_reservation (id, owner_kind, owner_id, sources, state, expires_at)
             VALUES (?, ?, ?, ?, 'pending', ?)
             ON CONFLICT DO NOTHING",
        )
        .bind(reservation.id.0.to_vec())
        .bind(owner_kind)
        .bind(owner_id.to_vec())
        .bind(sources)
        .bind(i64_of(reservation.expires_at)?)
        .execute(&mut *transaction)
        .await
        .map_err(db)?
        .rows_affected();
        if inserted == 1 {
            transaction.commit().await.map_err(db)?;
            return Ok(());
        }

        let stored: (String, Vec<u8>, String, String, i64) = sqlx::query_as(
            "SELECT owner_kind, owner_id, sources, state, expires_at
             FROM ledger_reservation WHERE id = ?",
        )
        .bind(reservation.id.0.to_vec())
        .fetch_one(&mut *transaction)
        .await
        .map_err(db)?;
        let stored_sources: Vec<ReservationSource> = serde_json::from_str(&stored.2).map_err(db)?;
        let exact = stored.3 == "pending"
            && stored.0 == owner_kind
            && b256(&stored.1)? == owner_id
            && stored_sources == reservation.sources
            && stored.4 == i64_of(reservation.expires_at)?;
        transaction.commit().await.map_err(db)?;
        match exact {
            true => Ok(()),
            false => Err(LedgerStoreError::Conflict(reservation.id)),
        }
    }

    async fn commit(&self, id: ReservationId) -> Result<(), LedgerStoreError> {
        self.transition(id, "committed", "pending").await
    }

    async fn post(&self, id: ReservationId, filled: &[U256]) -> Result<(), LedgerStoreError> {
        let filled = serde_json::to_string(filled).map_err(db)?;
        sqlx::query(
            "UPDATE ledger_reservation SET state = 'posted', filled = ?
             WHERE id = ? AND state IN ('pending', 'committed')",
        )
        .bind(filled)
        .bind(id.0.to_vec())
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn void(&self, id: ReservationId) -> Result<(), LedgerStoreError> {
        sqlx::query(
            "UPDATE ledger_reservation SET state = 'voided'
             WHERE id = ? AND state IN ('pending', 'committed')",
        )
        .bind(id.0.to_vec())
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn expire(&self, id: ReservationId) -> Result<(), LedgerStoreError> {
        self.transition(id, "expired", "pending").await
    }

    async fn void_reorg(&self, id: ReservationId) -> Result<(), LedgerStoreError> {
        self.transition(id, "reorg_open", "posted").await
    }

    async fn open_reservations(&self) -> Result<Vec<Reservation>, LedgerStoreError> {
        let rows: Vec<(Vec<u8>, String, Vec<u8>, String, String, i64)> = sqlx::query_as(
            "SELECT id, owner_kind, owner_id, sources, state, expires_at FROM ledger_reservation
             WHERE state IN ('pending', 'committed') ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        rows.into_iter()
            .map(|(id, owner_kind, owner_id, sources, state, expires_at)| {
                let sources: Vec<ReservationSource> = serde_json::from_str(&sources).map_err(db)?;
                let owner_id = b256(&owner_id)?;
                let owner = match owner_kind.as_str() {
                    "swap" => ReservationOwner::Swap(IntentId(owner_id)),
                    "rebate" => ReservationOwner::Rebate(RebateBatchId(owner_id)),
                    value => {
                        return Err(LedgerStoreError::Db(format!(
                            "unknown reservation owner kind '{value}'"
                        )))
                    }
                };
                let mut reservation =
                    Reservation::new(ReservationId(b256(&id)?), owner, sources, expires_at as u64);
                reservation.state = match state.as_str() {
                    "pending" => solvent_core::primitives::ledger::ReservationState::Pending,
                    "committed" => solvent_core::primitives::ledger::ReservationState::Committed,
                    _ => return Err(LedgerStoreError::Db(format!("invalid open state {state}"))),
                };
                Ok(reservation)
            })
            .collect()
    }
}
