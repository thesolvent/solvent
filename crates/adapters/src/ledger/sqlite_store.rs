//! The SQLite ledger store (sqlx), behind the storage-agnostic `LedgerStore` port. In-process, no
//! network hop. Each command is a single statement — atomic on its own in SQLite — that mirrors a
//! pure-engine command. `reserve` inserts idempotently on the reservation id; the transitions only
//! fire from the expected prior state, so a crash-replay converges instead of double-counting.

use alloy::primitives::{B256, U256};
use async_trait::async_trait;
use solvent_core::{
    deps::ledger::{LedgerStore, LedgerStoreError},
    primitives::{
        ledger::{Reservation, ReservationSource},
        IntentId, ReservationId,
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
        sqlx::query(
            "INSERT INTO ledger_reservation (id, intent, sources, state, expires_at)
             VALUES (?, ?, ?, 'pending', ?)
             ON CONFLICT DO NOTHING",
        )
        .bind(reservation.id.0.to_vec())
        .bind(reservation.intent.0.to_vec())
        .bind(sources)
        .bind(i64_of(reservation.expires_at)?)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn post(&self, id: ReservationId, filled: &[U256]) -> Result<(), LedgerStoreError> {
        let filled = serde_json::to_string(filled).map_err(db)?;
        sqlx::query(
            "UPDATE ledger_reservation SET state = 'posted', filled = ?
             WHERE id = ? AND state = 'pending'",
        )
        .bind(filled)
        .bind(id.0.to_vec())
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn void(&self, id: ReservationId) -> Result<(), LedgerStoreError> {
        self.transition(id, "voided", "pending").await
    }

    async fn expire(&self, id: ReservationId) -> Result<(), LedgerStoreError> {
        self.transition(id, "expired", "pending").await
    }

    async fn void_reorg(&self, id: ReservationId) -> Result<(), LedgerStoreError> {
        self.transition(id, "reorg_open", "posted").await
    }

    async fn open_reservations(&self) -> Result<Vec<Reservation>, LedgerStoreError> {
        let rows: Vec<(Vec<u8>, Vec<u8>, String, i64)> = sqlx::query_as(
            "SELECT id, intent, sources, expires_at FROM ledger_reservation
             WHERE state = 'pending' ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        rows.into_iter()
            .map(|(id, intent, sources, expires_at)| {
                let sources: Vec<ReservationSource> = serde_json::from_str(&sources).map_err(db)?;
                Ok(Reservation::new(
                    ReservationId(b256(&id)?),
                    IntentId(b256(&intent)?),
                    sources,
                    expires_at as u64,
                ))
            })
            .collect()
    }
}
