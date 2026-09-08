//! The SQLite recapture store (sqlx) behind the `RecaptureStore` port. Credits are keyed by
//! (intent, maker, token) so a re-driven reconcile accrues each fill once; the payout worker flips
//! `settled` when it has rebated them. `amount` is a decimal base-unit string (lossless).

use alloy::primitives::{Address, B256, U256};
use async_trait::async_trait;
use solvent_core::{
    deps::recapture::{RecaptureStore, RecaptureStoreError},
    primitives::{
        recapture::{AccruedCredit, RecaptureCredit},
        IntentId, MakerId,
    },
};
use sqlx::SqlitePool;

pub struct SqliteRecaptureStore {
    pool: SqlitePool,
}

impl SqliteRecaptureStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Apply the embedded schema migrations.
    pub async fn migrate(&self) -> Result<(), RecaptureStoreError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(db)
    }
}

fn db(e: impl std::fmt::Display) -> RecaptureStoreError {
    RecaptureStoreError::Write(e.to_string())
}

fn b256(bytes: &[u8]) -> Result<B256, RecaptureStoreError> {
    B256::try_from(bytes).map_err(|_| {
        RecaptureStoreError::Write(format!("expected a 32-byte id, got {}", bytes.len()))
    })
}

fn address(bytes: &[u8]) -> Result<Address, RecaptureStoreError> {
    Address::try_from(bytes).map_err(|_| {
        RecaptureStoreError::Write(format!("expected a 20-byte address, got {}", bytes.len()))
    })
}

fn amount(text: &str) -> Result<U256, RecaptureStoreError> {
    text.parse::<U256>()
        .map_err(|_| RecaptureStoreError::Write(format!("bad amount {text}")))
}

#[async_trait]
impl RecaptureStore for SqliteRecaptureStore {
    async fn accrue(
        &self,
        intent: IntentId,
        credits: &[RecaptureCredit],
    ) -> Result<(), RecaptureStoreError> {
        for credit in credits {
            sqlx::query(
                "INSERT INTO recapture_credit (intent, maker, token, amount)
                 VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING",
            )
            .bind(intent.0.to_vec())
            .bind(credit.maker.0.to_vec())
            .bind(credit.token.to_vec())
            .bind(credit.amount.to_string())
            .execute(&self.pool)
            .await
            .map_err(db)?;
        }
        Ok(())
    }

    async fn outstanding(&self) -> Result<Vec<AccruedCredit>, RecaptureStoreError> {
        let rows: Vec<(Vec<u8>, Vec<u8>, Vec<u8>, String)> = sqlx::query_as(
            "SELECT intent, maker, token, amount FROM recapture_credit
             WHERE settled = 0 ORDER BY intent, maker, token",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        rows.into_iter()
            .map(|(intent, maker, token, amt)| {
                Ok(AccruedCredit::new(
                    IntentId(b256(&intent)?),
                    RecaptureCredit::new(
                        MakerId(address(&maker)?),
                        address(&token)?,
                        amount(&amt)?,
                    ),
                ))
            })
            .collect()
    }

    async fn mark_settled(&self, credits: &[AccruedCredit]) -> Result<(), RecaptureStoreError> {
        // One transaction so a group's rows settle all-or-nothing: a failure mid-loop must never leave
        // part of an already-paid (maker, token) group outstanding for the next sweep to pay again.
        let mut tx = self.pool.begin().await.map_err(db)?;
        for accrued in credits {
            sqlx::query(
                "UPDATE recapture_credit SET settled = 1
                 WHERE intent = ? AND maker = ? AND token = ?",
            )
            .bind(accrued.intent.0.to_vec())
            .bind(accrued.credit.maker.0.to_vec())
            .bind(accrued.credit.token.to_vec())
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }
        tx.commit().await.map_err(db)?;
        Ok(())
    }
}
