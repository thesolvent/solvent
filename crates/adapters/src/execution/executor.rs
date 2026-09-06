//! `WalletkitExecutor` — the live [`Execution`] port over the walletkit tx engine: build the fill
//! call, submit it on the executor's fixed route, and project the engine's lifecycle onto the
//! terminal signal the ledger coupling needs.
//!
//! Tracking is durable but delegated: walletkit's own [`HandleId`] can't be rebuilt from raw bytes,
//! so each submitted fill's `(order_hash → reservation, handle_id)` is persisted through the
//! [`FillStore`] port (a serialized handle, opaque to that store). That store is the single source of
//! truth for the in-flight set: `status` resolves the handle from it, and after a restart `tracked`
//! is exactly the fills to recover (walletkit itself recovers and rebroadcasts its txs from its own
//! durable store on the next `tick`).

use std::sync::Arc;

use alloy::primitives::{B256, U256};
use async_trait::async_trait;
use walletkit::core::deps::SubmissionOpts;
use walletkit::core::wallet::{HandleId, SimOutcome, TxIntent, TxStatus};
use walletkit::Wallet;

use solvent_core::deps::execution::{Execution, ExecutionError, FillStore, SimError, SimGate};
use solvent_core::primitives::execution::{
    ExecHandle, ExecStatus, FillTx, SimVerdict, TrackedFill,
};
use solvent_core::primitives::{IntentId, ReservationId};

/// Submits and tracks fills through one walletkit [`Wallet`]. `submission` fixes the broadcast route
/// for every fill: a private relay (Flashbots/Protect) in production, or the public mempool on a
/// local chain with no relay. Durable tracking lives behind the [`FillStore`] port.
pub struct WalletkitExecutor {
    wallet: Wallet,
    submission: SubmissionOpts,
    store: Arc<dyn FillStore>,
}

impl WalletkitExecutor {
    pub fn new(wallet: Wallet, submission: SubmissionOpts, store: Arc<dyn FillStore>) -> Self {
        Self {
            wallet,
            submission,
            store,
        }
    }
}

fn engine(e: impl std::fmt::Display) -> ExecutionError {
    ExecutionError::Engine(e.to_string())
}

/// Build the fill's raw call — a zero-value call to the filler contract from its owner. Shared by
/// submission and simulation so both send byte-identical calldata to the same target.
fn fill_to_intent(fill: &FillTx) -> TxIntent {
    TxIntent::call(
        fill.chain_id,
        fill.filler_owner,
        fill.filler,
        U256::ZERO,
        fill.calldata.clone(),
    )
}

/// Project the engine's lifecycle onto the reservation signal. Terminal states drive the ledger:
/// `Confirmed` posts, and the two "our fill did not land" terminals (`Replaced`/`Dropped`) void.
/// Everything else keeps the fill in flight — the engine absorbs sub-confirmation reorgs by falling
/// back to earlier states, so no non-terminal state is final and an unknown future one is not either.
fn to_exec_status(status: TxStatus, mined_tx: B256) -> ExecStatus {
    match status {
        TxStatus::Confirmed { block } => ExecStatus::Confirmed {
            block,
            tx: mined_tx,
        },
        TxStatus::Failed { reason } => ExecStatus::Failed { reason },
        TxStatus::Replaced | TxStatus::Dropped => ExecStatus::Dropped,
        _ => ExecStatus::Pending,
    }
}

/// Turn a simulation outcome into a submit/drop verdict. Fail-closed: only a confirmed `Success`
/// passes; a revert (the P1 filler's own guards — under-delivery, stale caps, profit threshold —
/// surface here) and any outcome the engine can't classify both reject before a nonce is spent.
fn to_verdict(outcome: SimOutcome) -> SimVerdict {
    match outcome {
        SimOutcome::Success => SimVerdict::Ok,
        SimOutcome::Revert(reason) => SimVerdict::Reject {
            reason: format!("{reason:?}"),
        },
        _ => SimVerdict::Reject {
            reason: "unrecognized simulation outcome".into(),
        },
    }
}

#[async_trait]
impl Execution for WalletkitExecutor {
    async fn submit(
        &self,
        fill: &FillTx,
        reservation: ReservationId,
    ) -> Result<ExecHandle, ExecutionError> {
        let key = fill.intent.0;
        // Idempotent: a tracked intent returns its handle without spending a second nonce.
        if self
            .store
            .handle(fill.intent)
            .await
            .map_err(engine)?
            .is_some()
        {
            return Ok(ExecHandle(key));
        }

        let handle = self
            .wallet
            .send_with(&fill_to_intent(fill), self.submission.clone())
            .await
            .map_err(engine)?;
        let handle_id = serde_json::to_vec(&handle.id).map_err(engine)?;
        self.store
            .track(fill.intent, reservation, &handle_id)
            .await
            .map_err(engine)?;
        Ok(ExecHandle(key))
    }

    async fn status(&self, handle: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError> {
        let Some(bytes) = self
            .store
            .handle(IntentId(handle.0))
            .await
            .map_err(engine)?
        else {
            return Ok(None);
        };
        let id: HandleId = serde_json::from_slice(&bytes).map_err(engine)?;
        // Read the full handle, not just the status: the mined hash lives in `broadcasts` (its last
        // entry survives an RBF bump), and the settlement reader keys off it.
        let tracked = self.wallet.handle(id).await.map_err(engine)?;
        Ok(tracked.map(|h| {
            let mined = h.broadcasts.last().copied().unwrap_or_default();
            to_exec_status(h.status, mined)
        }))
    }

    async fn forget(&self, intent: IntentId) -> Result<(), ExecutionError> {
        self.store.untrack(intent).await.map_err(engine)
    }

    async fn tracked(&self) -> Result<Vec<TrackedFill>, ExecutionError> {
        self.store.tracked().await.map_err(engine)
    }

    async fn tick(&self) -> Result<(), ExecutionError> {
        self.wallet.tick().await.map_err(engine)
    }
}

#[async_trait]
impl SimGate for WalletkitExecutor {
    async fn simulate(&self, fill: &FillTx) -> Result<SimVerdict, SimError> {
        let intent = fill_to_intent(fill);
        let preview = self
            .wallet
            .dry_run(&intent)
            .await
            .map_err(|e| SimError::Engine(e.to_string()))?;
        Ok(to_verdict(preview.outcome))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_projection_decides_post_vs_void() {
        let tx = B256::from([9; 32]);
        assert_eq!(
            to_exec_status(TxStatus::Confirmed { block: 7 }, tx),
            ExecStatus::Confirmed { block: 7, tx }
        );
        assert_eq!(
            to_exec_status(
                TxStatus::Failed {
                    reason: "revert".into()
                },
                tx
            ),
            ExecStatus::Failed {
                reason: "revert".into()
            }
        );
        assert_eq!(to_exec_status(TxStatus::Dropped, tx), ExecStatus::Dropped);
        assert_eq!(to_exec_status(TxStatus::Replaced, tx), ExecStatus::Dropped);
        for in_flight in [
            TxStatus::Pending,
            TxStatus::Sent,
            TxStatus::Mined {
                block: 1,
                block_hash: B256::ZERO,
            },
            TxStatus::Replacing { since_block: 1 },
        ] {
            assert_eq!(to_exec_status(in_flight, tx), ExecStatus::Pending);
        }
    }

    #[test]
    fn sim_gate_passes_only_a_confirmed_success() {
        use walletkit::core::wallet::RevertReason;

        assert_eq!(to_verdict(SimOutcome::Success), SimVerdict::Ok);
        // A revert (the filler's on-chain guards fire here) drops the fill, keeping the reason.
        assert!(matches!(
            to_verdict(SimOutcome::Revert(RevertReason::Error(
                "insufficient output".into()
            ))),
            SimVerdict::Reject { .. }
        ));
    }
}
