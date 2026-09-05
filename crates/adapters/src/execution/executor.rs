//! `WalletkitExecutor` — the live [`Execution`] port over the walletkit tx engine: build the fill
//! call, submit it on the executor's fixed route, and project the engine's lifecycle onto the
//! terminal signal the ledger coupling needs.

use std::collections::HashMap;

use alloy::primitives::{B256, U256};
use async_trait::async_trait;
use parking_lot::Mutex;
use walletkit::core::deps::SubmissionOpts;
use walletkit::core::wallet::{HandleId, TxIntent, TxStatus};
use walletkit::Wallet;

use solvent_core::deps::execution::{Execution, ExecutionError};
use solvent_core::primitives::execution::{ExecHandle, ExecStatus, FillTx};

/// Submits and tracks fills through one walletkit [`Wallet`]. `submission` fixes the broadcast
/// route for every fill: a private relay (Flashbots/Protect) in production, or the public mempool
/// on a local chain with no relay. The engine's own handle id can't be rebuilt from raw bytes, so
/// each submission's id is remembered here and looked up on `status`.
pub struct WalletkitExecutor {
    wallet: Wallet,
    submission: SubmissionOpts,
    handles: Mutex<HashMap<B256, HandleId>>,
}

impl WalletkitExecutor {
    pub fn new(wallet: Wallet, submission: SubmissionOpts) -> Self {
        Self {
            wallet,
            submission,
            handles: Mutex::new(HashMap::new()),
        }
    }
}

/// Project the engine's lifecycle onto the reservation signal. Terminal states drive the ledger:
/// `Confirmed` posts, and the two "our fill did not land" terminals (`Replaced`/`Dropped`) void.
/// Everything else keeps the fill in flight — the engine absorbs sub-confirmation reorgs by falling
/// back to earlier states, so no non-terminal state is final and an unknown future one is not either.
fn to_exec_status(status: TxStatus) -> ExecStatus {
    match status {
        TxStatus::Confirmed { block } => ExecStatus::Confirmed { block },
        TxStatus::Failed { reason } => ExecStatus::Failed { reason },
        TxStatus::Replaced | TxStatus::Dropped => ExecStatus::Dropped,
        _ => ExecStatus::Pending,
    }
}

#[async_trait]
impl Execution for WalletkitExecutor {
    async fn submit(&self, fill: &FillTx) -> Result<ExecHandle, ExecutionError> {
        let intent = TxIntent::call(
            fill.chain_id,
            fill.filler_owner,
            fill.filler,
            U256::ZERO,
            fill.calldata.clone(),
        );
        let handle = self
            .wallet
            .send_with(&intent, self.submission.clone())
            .await
            .map_err(|e| ExecutionError::Engine(e.to_string()))?;
        let key = fill.intent.0;
        self.handles.lock().insert(key, handle.id);
        Ok(ExecHandle(key))
    }

    async fn status(&self, handle: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError> {
        let id = self.handles.lock().get(&handle.0).copied();
        match id {
            Some(id) => {
                let status = self
                    .wallet
                    .status(id)
                    .await
                    .map_err(|e| ExecutionError::Engine(e.to_string()))?;
                Ok(status.map(to_exec_status))
            }
            None => Ok(None),
        }
    }

    async fn tick(&self) -> Result<(), ExecutionError> {
        self.wallet
            .tick()
            .await
            .map_err(|e| ExecutionError::Engine(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_projection_decides_post_vs_void() {
        assert_eq!(
            to_exec_status(TxStatus::Confirmed { block: 7 }),
            ExecStatus::Confirmed { block: 7 }
        );
        assert_eq!(
            to_exec_status(TxStatus::Failed {
                reason: "revert".into()
            }),
            ExecStatus::Failed {
                reason: "revert".into()
            }
        );
        assert_eq!(to_exec_status(TxStatus::Dropped), ExecStatus::Dropped);
        assert_eq!(to_exec_status(TxStatus::Replaced), ExecStatus::Dropped);
        for in_flight in [
            TxStatus::Pending,
            TxStatus::Sent,
            TxStatus::Mined {
                block: 1,
                block_hash: B256::ZERO,
            },
            TxStatus::Replacing { since_block: 1 },
        ] {
            assert_eq!(to_exec_status(in_flight), ExecStatus::Pending);
        }
    }
}
