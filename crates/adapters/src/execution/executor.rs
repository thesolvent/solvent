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

use alloy::{
    primitives::B256,
    providers::{DynProvider, Provider},
};
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
    provider: DynProvider,
    submission: SubmissionOpts,
    store: Arc<dyn FillStore>,
}

impl WalletkitExecutor {
    pub fn new(
        wallet: Wallet,
        provider: DynProvider,
        submission: SubmissionOpts,
        store: Arc<dyn FillStore>,
    ) -> Self {
        Self {
            wallet,
            provider,
            submission,
            store,
        }
    }

    async fn status_for(
        &self,
        handle: walletkit::core::wallet::TxHandle,
    ) -> Result<ExecStatus, ExecutionError> {
        let status = match handle.status {
            TxStatus::Confirmed { block } => ExecStatus::Confirmed {
                block,
                tx: self.mined_broadcast(&handle.broadcasts, block).await?,
            },
            status => to_exec_status(
                status,
                handle.broadcasts.last().copied().unwrap_or_default(),
            ),
        };
        Ok(status)
    }

    async fn mined_broadcast(
        &self,
        broadcasts: &[B256],
        block: u64,
    ) -> Result<B256, ExecutionError> {
        for tx in broadcasts.iter().rev() {
            let receipt = self
                .provider
                .get_transaction_receipt(*tx)
                .await
                .map_err(engine)?;
            if receipt.is_some_and(|receipt| receipt.block_number == Some(block)) {
                return Ok(*tx);
            }
        }
        Err(ExecutionError::Engine(format!(
            "walletkit confirmed fill has no receipt at block {block}"
        )))
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
        fill.value,
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
/// passes; a revert (the filler contract's own on-chain guards — under-delivery, stale caps, profit
/// threshold — surface here) and any outcome the engine can't classify both reject before a nonce
/// is spent.
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
            return self
                .store
                .terminal(IntentId(handle.0))
                .await
                .map_err(engine);
        };
        let id: HandleId = serde_json::from_slice(&bytes).map_err(engine)?;
        // Settlement needs the broadcast whose receipt mined, not just WalletKit's finalized block.
        let tracked = self.wallet.handle(id).await.map_err(engine)?;
        match tracked {
            Some(h) => Ok(Some(self.status_for(h).await?)),
            None => self
                .store
                .terminal(IntentId(handle.0))
                .await
                .map_err(engine),
        }
    }

    async fn forget(&self, intent: IntentId) -> Result<(), ExecutionError> {
        if let Some(bytes) = self.store.handle(intent).await.map_err(engine)? {
            let id: HandleId = serde_json::from_slice(&bytes).map_err(engine)?;
            if let Some(handle) = self.wallet.handle(id).await.map_err(engine)? {
                let status = self.status_for(handle).await?;
                self.store
                    .record_terminal(intent, &status)
                    .await
                    .map_err(engine)?;
            }
        }
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
    use alloy::{
        network::{EthereumWallet, TransactionBuilder},
        node_bindings::Anvil,
        primitives::{Address, Bytes, U256},
        providers::{Provider, ProviderBuilder},
        rpc::types::TransactionRequest,
        signers::local::PrivateKeySigner,
    };
    use sqlx::SqlitePool;
    use tempfile::tempdir;
    use walletkit::{
        adapters::{
            policy::{AllowAll, DefaultPolicyEngine},
            LocalSigner, RedbStateStore, SystemClock, Transport,
        },
        core::{
            deps::StateStore,
            wallet::{GasEnvelope, TxHandle},
        },
    };

    use crate::execution::SqliteFillStore;

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

    #[tokio::test]
    async fn status_uses_the_broadcast_with_the_mined_receipt() {
        let anvil = Anvil::new().try_spawn().expect("spawn anvil");
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let account = signer.address();
        let provider = ProviderBuilder::new()
            .wallet(EthereumWallet::from(signer.clone()))
            .connect_http(anvil.endpoint_url());
        let receipt = provider
            .send_transaction(
                TransactionRequest::default()
                    .with_to(Address::repeat_byte(0x42))
                    .with_value(U256::from(1)),
            )
            .await
            .expect("send original")
            .get_receipt()
            .await
            .expect("mine original");
        let original = receipt.transaction_hash;
        let block = receipt.block_number.expect("receipt block");
        let bump = B256::repeat_byte(0x99);

        let dir = tempdir().expect("tempdir");
        let state_store = Arc::new(
            RedbStateStore::open(dir.path().join("wallet.redb")).expect("open state store"),
        );
        let intent = TxIntent::transfer(31337, account, Address::repeat_byte(0x42), U256::from(1));
        let intent_hash = intent.hash();
        let handle = TxHandle {
            id: HandleId::new(intent_hash, 0),
            account,
            intent,
            intent_hash,
            nonce: 0,
            status: TxStatus::Confirmed { block },
            envelope: GasEnvelope::DEFAULT,
            signed: Bytes::new(),
            broadcasts: vec![original, bump],
            last_broadcast_at: 0,
            cancelled: false,
            submission: SubmissionOpts::public(),
            meta: None,
        };
        state_store.put_handle(&handle).await.expect("store handle");

        let database_url = format!(
            "sqlite://{}?mode=rwc",
            dir.path().join("fills.db").display()
        );
        let pool = SqlitePool::connect(&database_url)
            .await
            .expect("open fill store");
        let fill_store = Arc::new(SqliteFillStore::new(pool));
        fill_store.migrate().await.expect("migrate fill store");
        let fill_intent = IntentId(B256::repeat_byte(0x01));
        fill_store
            .track(
                fill_intent,
                ReservationId(B256::repeat_byte(0x02)),
                &serde_json::to_vec(&handle.id).expect("serialize handle id"),
            )
            .await
            .expect("track fill");

        let key = format!("0x{}", alloy::hex::encode(signer.to_bytes()));
        let wallet_signer = LocalSigner::from_private_key(&key).expect("wallet signer");
        let transport = Transport::url(anvil.endpoint_url()).expect("transport");
        let policy = DefaultPolicyEngine::new(vec![Box::new(AllowAll)], Arc::new(SystemClock));
        let wallet = Wallet::builder(
            Arc::new(transport),
            Arc::new(wallet_signer),
            Arc::new(policy),
        )
        .store(state_store)
        .confirmations(1)
        .build();
        let executor = WalletkitExecutor::new(
            wallet,
            provider.erased(),
            SubmissionOpts::public(),
            fill_store,
        );

        assert_eq!(
            executor
                .status(ExecHandle(fill_intent.0))
                .await
                .expect("execution status"),
            Some(ExecStatus::Confirmed {
                block,
                tx: original
            }),
            "a receiptless later bump must not replace the hash used for settlement",
        );
    }
}
