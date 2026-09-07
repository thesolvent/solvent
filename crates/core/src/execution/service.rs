//! The execution service: the last leg of the intent lifecycle. `fill` simulates a reserved plan and
//! submits it if it passes (voiding the reservation on a reject, before a nonce is spent);
//! `reconcile` advances the tx engine and settles each terminal fill with the *actual* per-source
//! amounts, or voids on failure. The in-flight set lives in the durable engine, not memory (via
//! [`Execution::tracked`]), so a restart recovers every submitted fill through the same path.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::deps::execution::{Execution, SettlementReader, SimGate};
use crate::ledger::LedgerService;
use crate::obs::{info, warn};
use crate::primitives::execution::{
    ExecHandle, ExecStatus, FillOutcome, PendingFill, Settled, SettledOutcome, SimVerdict,
};
use crate::primitives::ledger::LedgerError;
use crate::primitives::{ReservationId, SolventError};

pub struct ExecutionService {
    sim: Arc<dyn SimGate>,
    execution: Arc<dyn Execution>,
    settlement: Arc<dyn SettlementReader>,
    ledger: Arc<LedgerService>,
}

impl ExecutionService {
    pub fn new(
        sim: Arc<dyn SimGate>,
        execution: Arc<dyn Execution>,
        settlement: Arc<dyn SettlementReader>,
        ledger: Arc<LedgerService>,
    ) -> Self {
        Self {
            sim,
            execution,
            settlement,
            ledger,
        }
    }

    /// Simulate then privately submit a reserved plan. A simulation `Reject` voids the reservation
    /// and returns without spending a nonce; submission is idempotent on the intent — resubmitting
    /// a tracked fill returns its handle, so one order never fills twice.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(intent = %pending.fill_tx.intent)))]
    pub async fn fill(&self, pending: PendingFill) -> Result<FillOutcome, SolventError> {
        match self.sim.simulate(&pending.fill_tx).await? {
            SimVerdict::Reject { reason } => {
                warn!("fill dropped by sim gate; voiding reservation");
                self.ledger.void(pending.reservation).await?;
                Ok(FillOutcome::Rejected { reason })
            }
            SimVerdict::Ok => {
                let handle = self
                    .execution
                    .submit(&pending.fill_tx, pending.reservation)
                    .await?;
                info!("fill submitted");
                Ok(FillOutcome::Submitted { handle })
            }
        }
    }

    /// Advance the tx engine, then settle every tracked fill that reached a terminal state: on
    /// `Confirmed`, post the actual amounts the fill pulled (read from its own settlement) and drop
    /// the tracking; on failure, void and drop. Returns the fills that reached a terminal state so
    /// the caller can settle the trade lifecycle and drive recapture. Each intent is isolated — a
    /// transient error on one is logged and left tracked to retry, never dropping the fills already
    /// collected for the others — and a reservation a redelivery already settled is a no-op, so this
    /// is safe to call repeatedly; after a restart it recovers the durably tracked fills through
    /// this same path.
    pub async fn reconcile(&self) -> Result<Vec<Settled>, SolventError> {
        self.execution.tick().await?;

        let mut settled = Vec::new();
        for f in self.execution.tracked().await? {
            // Each intent settles on its own: a transient error on one (a status or receipt read) is
            // logged and left tracked to retry, never aborting the pass and discarding the fills
            // already collected for its siblings.
            let status = match self.execution.status(ExecHandle(f.intent.0)).await {
                Ok(Some(status)) => status,
                Ok(None) => continue,
                Err(_) => {
                    warn!(intent = %f.intent, "fill status unreadable; retrying next cycle");
                    continue;
                }
            };
            let outcome = match status {
                ExecStatus::Confirmed { tx, block } => {
                    match self.ledger.reservation_sources(f.reservation).await {
                        Some(sources) => {
                            let Ok(filled) = self.settlement.settled(tx, &sources).await else {
                                warn!(intent = %f.intent, "settlement unreadable; retrying next cycle");
                                continue;
                            };
                            if settle(self.ledger.post(f.reservation, &filled).await).is_err() {
                                warn!(intent = %f.intent, "posting the confirmed fill failed; retrying next cycle");
                                continue;
                            }
                            info!(intent = %f.intent, "fill confirmed; reservation posted");
                        }
                        None => {
                            warn!(intent = %f.intent, "fill confirmed but reservation is gone; skipping post");
                        }
                    }
                    SettledOutcome::Confirmed { tx, block }
                }
                ExecStatus::Failed { .. } | ExecStatus::Dropped => {
                    if settle(self.ledger.void(f.reservation).await).is_err() {
                        warn!(intent = %f.intent, "voiding the failed fill failed; retrying next cycle");
                        continue;
                    }
                    warn!(intent = %f.intent, "fill did not land; reservation voided");
                    SettledOutcome::Failed
                }
                ExecStatus::Pending => continue,
            };
            // Still tracked means it is reported again next cycle; the ledger and trade FSMs make
            // that replay a no-op, so a failed drop costs a retry, never a lost settlement.
            if self.execution.forget(f.intent).await.is_err() {
                warn!(intent = %f.intent, "settled fill still tracked; retrying next cycle");
                continue;
            }
            settled.push(Settled {
                intent: f.intent,
                outcome,
            });
        }
        Ok(settled)
    }

    /// Reverse a posted fill a chain reorg rolled back — the compensating ledger transition. The
    /// caller supplies the reorg signal (detecting a post-finality un-mine against the canonical
    /// chain is the reconcile worker's job, not this method's).
    pub async fn on_reorg(&self, reservation: ReservationId) -> Result<(), SolventError> {
        self.ledger.void_reorg(reservation).await
    }

    /// How many fills are still in flight — zero once every submitted fill has settled.
    pub async fn pending(&self) -> Result<usize, SolventError> {
        Ok(self.execution.tracked().await?.len())
    }

    /// The reservations of every fill still in flight — the set a TTL sweep must not touch, since
    /// their transactions can still land.
    pub async fn tracked_reservations(&self) -> Result<BTreeSet<ReservationId>, SolventError> {
        Ok(self
            .execution
            .tracked()
            .await?
            .iter()
            .map(|f| f.reservation)
            .collect())
    }
}

/// Treat an already-terminal reservation as done, not an error: the ledger FSM is the one-shot
/// guard, so a terminal that a redelivery already posted or voided settles as a no-op here.
fn settle(result: Result<(), SolventError>) -> Result<(), SolventError> {
    match result {
        Err(SolventError::Ledger(LedgerError::WrongState { .. })) => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;

    use alloy_primitives::{Address, Bytes, B256, U256};
    use async_trait::async_trait;

    use crate::deps::execution::{ExecutionError, SettlementError, SimError};
    use crate::deps::ledger::{
        BudgetSource, BudgetSourceError, Clock, LedgerStore, LedgerStoreError,
    };
    use crate::primitives::execution::{FillTx, TrackedFill};
    use crate::primitives::ledger::{AccountKey, Reservation, ReservationSource};
    use crate::primitives::{IntentId, MakerId, StrategyHash};

    struct FakeSim(SimVerdict);
    #[async_trait]
    impl SimGate for FakeSim {
        async fn simulate(&self, _: &FillTx) -> Result<SimVerdict, SimError> {
            Ok(self.0.clone())
        }
    }

    struct FakeExec {
        status: ExecStatus,
        submits: AtomicUsize,
        tracked: StdMutex<HashMap<IntentId, ReservationId>>,
    }
    impl FakeExec {
        fn new(status: ExecStatus) -> Self {
            Self {
                status,
                submits: AtomicUsize::new(0),
                tracked: StdMutex::new(HashMap::new()),
            }
        }
    }
    #[async_trait]
    impl Execution for FakeExec {
        async fn submit(
            &self,
            fill: &FillTx,
            reservation: ReservationId,
        ) -> Result<ExecHandle, ExecutionError> {
            self.tracked
                .lock()
                .unwrap()
                .entry(fill.intent)
                .or_insert_with(|| {
                    self.submits.fetch_add(1, Ordering::Relaxed);
                    reservation
                });
            Ok(ExecHandle(fill.intent.0))
        }
        async fn status(&self, handle: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError> {
            let tracked = self.tracked.lock().unwrap();
            Ok(tracked
                .contains_key(&IntentId(handle.0))
                .then(|| self.status.clone()))
        }
        async fn forget(&self, intent: IntentId) -> Result<(), ExecutionError> {
            self.tracked.lock().unwrap().remove(&intent);
            Ok(())
        }
        async fn tracked(&self) -> Result<Vec<TrackedFill>, ExecutionError> {
            Ok(self
                .tracked
                .lock()
                .unwrap()
                .iter()
                .map(|(intent, reservation)| TrackedFill::new(*intent, *reservation))
                .collect())
        }
        async fn tick(&self) -> Result<(), ExecutionError> {
            Ok(())
        }
    }

    struct FakeSettle(Vec<U256>);
    #[async_trait]
    impl SettlementReader for FakeSettle {
        async fn settled(
            &self,
            _: B256,
            _: &[ReservationSource],
        ) -> Result<Vec<U256>, SettlementError> {
            Ok(self.0.clone())
        }
    }

    struct FakeStore;
    #[async_trait]
    impl LedgerStore for FakeStore {
        async fn reserve(&self, _: &Reservation) -> Result<(), LedgerStoreError> {
            Ok(())
        }
        async fn post(&self, _: ReservationId, _: &[U256]) -> Result<(), LedgerStoreError> {
            Ok(())
        }
        async fn void(&self, _: ReservationId) -> Result<(), LedgerStoreError> {
            Ok(())
        }
        async fn expire(&self, _: ReservationId) -> Result<(), LedgerStoreError> {
            Ok(())
        }
        async fn void_reorg(&self, _: ReservationId) -> Result<(), LedgerStoreError> {
            Ok(())
        }
        async fn open_reservations(&self) -> Result<Vec<Reservation>, LedgerStoreError> {
            Ok(vec![])
        }
    }

    struct FakeBudget(U256);
    #[async_trait]
    impl BudgetSource for FakeBudget {
        async fn budget(&self, _: &AccountKey) -> Result<U256, BudgetSourceError> {
            Ok(self.0)
        }
    }

    struct FakeClock;
    impl Clock for FakeClock {
        fn now_unix(&self) -> u64 {
            1_000
        }
    }

    const MAKER: u8 = 7;
    const TOKEN: u8 = 9;

    fn maker() -> MakerId {
        MakerId(Address::from([MAKER; 20]))
    }
    fn wallet_account() -> AccountKey {
        AccountKey::WalletBudget {
            maker: maker(),
            token: Address::from([TOKEN; 20]),
        }
    }
    fn source(amount: u64) -> ReservationSource {
        ReservationSource {
            maker: maker(),
            strategy_hash: StrategyHash(B256::from([1; 32])),
            token: Address::from([TOKEN; 20]),
            amount: U256::from(amount),
        }
    }
    fn ids(n: u8) -> (IntentId, ReservationId) {
        (
            IntentId(B256::from([n; 32])),
            ReservationId(B256::from([n; 32])),
        )
    }
    fn pending(intent: IntentId, reservation: ReservationId) -> PendingFill {
        PendingFill::new(
            FillTx::new(
                intent,
                1,
                Address::from([0xaa; 20]),
                Address::from([0xbb; 20]),
                Bytes::new(),
            ),
            reservation,
        )
    }

    fn ledger(budget: u64) -> Arc<LedgerService> {
        Arc::new(LedgerService::new(
            Arc::new(FakeStore),
            Arc::new(FakeBudget(U256::from(budget))),
            Arc::new(FakeClock),
        ))
    }

    /// Build a service over a real ledger; hand back the exec fake (for submit counts) and the
    /// ledger (for `available` assertions).
    fn service(
        sim: SimVerdict,
        status: ExecStatus,
        settled: Vec<U256>,
        ledger: Arc<LedgerService>,
    ) -> (ExecutionService, Arc<FakeExec>) {
        let exec = Arc::new(FakeExec::new(status));
        let svc = ExecutionService::new(
            Arc::new(FakeSim(sim)),
            exec.clone(),
            Arc::new(FakeSettle(settled)),
            ledger,
        );
        (svc, exec)
    }

    fn confirmed() -> ExecStatus {
        ExecStatus::Confirmed {
            block: 1,
            tx: B256::from([2; 32]),
        }
    }

    #[tokio::test]
    async fn reject_voids_the_reservation_without_submitting() {
        let (intent, rid) = ids(1);
        let led = ledger(1000);
        led.reserve(rid, intent, vec![source(100)], 60)
            .await
            .unwrap();
        assert_eq!(led.available(&wallet_account()), U256::from(900u64));

        let (svc, exec) = service(
            SimVerdict::Reject {
                reason: "stale".into(),
            },
            confirmed(),
            vec![],
            led.clone(),
        );
        let out = svc.fill(pending(intent, rid)).await.unwrap();
        assert!(matches!(out, FillOutcome::Rejected { .. }));
        assert_eq!(
            exec.submits.load(Ordering::Relaxed),
            0,
            "never spent a nonce"
        );
        assert_eq!(
            led.available(&wallet_account()),
            U256::from(1000u64),
            "hold voided"
        );
    }

    #[tokio::test]
    async fn confirmed_fill_below_reserved_posts_actual_and_returns_remainder() {
        let (intent, rid) = ids(2);
        let led = ledger(1000);
        led.reserve(rid, intent, vec![source(100)], 60)
            .await
            .unwrap();
        assert_eq!(led.available(&wallet_account()), U256::from(900u64));

        // Reserved 100, but the fill pulled only 60.
        let (svc, _) = service(
            SimVerdict::Ok,
            confirmed(),
            vec![U256::from(60u64)],
            led.clone(),
        );
        svc.fill(pending(intent, rid)).await.unwrap();
        let settled = svc.reconcile().await.unwrap();

        assert!(matches!(
            settled.as_slice(),
            [Settled {
                outcome: SettledOutcome::Confirmed { .. },
                ..
            }]
        ));
        assert_eq!(svc.pending().await.unwrap(), 0);
        // 60 consumed, the 40 remainder returned to available.
        assert_eq!(led.available(&wallet_account()), U256::from(940u64));
    }

    #[tokio::test]
    async fn failed_fill_voids_the_reservation() {
        let (intent, rid) = ids(3);
        let led = ledger(1000);
        led.reserve(rid, intent, vec![source(100)], 60)
            .await
            .unwrap();

        let (svc, _) = service(
            SimVerdict::Ok,
            ExecStatus::Failed {
                reason: "revert".into(),
            },
            vec![],
            led.clone(),
        );
        svc.fill(pending(intent, rid)).await.unwrap();
        let settled = svc.reconcile().await.unwrap();

        assert!(matches!(
            settled.as_slice(),
            [Settled {
                outcome: SettledOutcome::Failed,
                ..
            }]
        ));
        assert_eq!(svc.pending().await.unwrap(), 0);
        assert_eq!(
            led.available(&wallet_account()),
            U256::from(1000u64),
            "hold released"
        );
    }

    #[tokio::test]
    async fn on_reorg_reverses_a_posted_fill() {
        let (intent, rid) = ids(4);
        let led = ledger(1000);
        led.reserve(rid, intent, vec![source(100)], 60)
            .await
            .unwrap();

        let (svc, _) = service(
            SimVerdict::Ok,
            confirmed(),
            vec![U256::from(100u64)],
            led.clone(),
        );
        svc.fill(pending(intent, rid)).await.unwrap();
        svc.reconcile().await.unwrap();
        assert_eq!(
            led.available(&wallet_account()),
            U256::from(900u64),
            "100 consumed"
        );

        svc.on_reorg(rid).await.unwrap();
        assert_eq!(
            led.available(&wallet_account()),
            U256::from(1000u64),
            "consumption reversed"
        );
    }

    #[tokio::test]
    async fn duplicate_fill_submits_once() {
        let (intent, rid) = ids(5);
        let led = ledger(1000);
        led.reserve(rid, intent, vec![source(100)], 60)
            .await
            .unwrap();

        let (svc, exec) = service(SimVerdict::Ok, confirmed(), vec![U256::from(100u64)], led);
        let a = svc.fill(pending(intent, rid)).await.unwrap();
        let b = svc.fill(pending(intent, rid)).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(
            exec.submits.load(Ordering::Relaxed),
            1,
            "idempotent by intent"
        );
    }

    #[tokio::test]
    async fn already_settled_reservation_is_a_noop() {
        let (intent, rid) = ids(6);
        let led = ledger(1000);
        led.reserve(rid, intent, vec![source(100)], 60)
            .await
            .unwrap();

        let (svc, _) = service(
            SimVerdict::Ok,
            confirmed(),
            vec![U256::from(100u64)],
            led.clone(),
        );
        svc.fill(pending(intent, rid)).await.unwrap();
        // A concurrent path already posted this reservation.
        led.post(rid, &[U256::from(100u64)]).await.unwrap();

        // reconcile sees Confirmed and tries to post again -> WrongState -> swallowed, not an error.
        svc.reconcile().await.unwrap();
        assert_eq!(svc.pending().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn reconcile_recovers_a_durably_tracked_fill_without_a_prior_fill() {
        // A restart: the reservation is open (the ledger recovered it) and the fill is durably
        // tracked (the engine recovered it), but this service instance never called `fill`.
        // Reconcile alone must still drive it to settlement.
        let (intent, rid) = ids(7);
        let led = ledger(1000);
        led.reserve(rid, intent, vec![source(100)], 60)
            .await
            .unwrap();

        let (svc, exec) = service(
            SimVerdict::Ok,
            confirmed(),
            vec![U256::from(100u64)],
            led.clone(),
        );
        // As restored from the durable tracking table on boot — no `fill`, no nonce spent.
        exec.tracked.lock().unwrap().insert(intent, rid);

        svc.reconcile().await.unwrap();
        assert_eq!(svc.pending().await.unwrap(), 0);
        assert_eq!(
            exec.submits.load(Ordering::Relaxed),
            0,
            "recovery never submits"
        );
        assert_eq!(
            led.available(&wallet_account()),
            U256::from(900u64),
            "recovered fill posted"
        );
    }

    /// Confirms one designated handle; every other handle's status read errors — models a transient
    /// per-intent RPC failure alongside a clean settlement.
    struct FlakyExec {
        good: ExecHandle,
        good_status: ExecStatus,
        tracked: StdMutex<HashMap<IntentId, ReservationId>>,
    }
    #[async_trait]
    impl Execution for FlakyExec {
        async fn submit(
            &self,
            fill: &FillTx,
            reservation: ReservationId,
        ) -> Result<ExecHandle, ExecutionError> {
            self.tracked
                .lock()
                .unwrap()
                .insert(fill.intent, reservation);
            Ok(ExecHandle(fill.intent.0))
        }
        async fn status(&self, handle: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError> {
            if handle == self.good {
                Ok(Some(self.good_status.clone()))
            } else {
                Err(ExecutionError::Engine("rpc down".into()))
            }
        }
        async fn forget(&self, intent: IntentId) -> Result<(), ExecutionError> {
            self.tracked.lock().unwrap().remove(&intent);
            Ok(())
        }
        async fn tracked(&self) -> Result<Vec<TrackedFill>, ExecutionError> {
            Ok(self
                .tracked
                .lock()
                .unwrap()
                .iter()
                .map(|(intent, reservation)| TrackedFill::new(*intent, *reservation))
                .collect())
        }
        async fn tick(&self) -> Result<(), ExecutionError> {
            Ok(())
        }
    }

    // A transient error on one tracked intent must not discard another's settled fill: A settles
    // cleanly while B's status read errors — A is still reported and B stays tracked to retry.
    #[tokio::test]
    async fn one_intents_error_does_not_drop_anothers_confirmed_fill() {
        let (intent_a, rid_a) = ids(1);
        let (intent_b, rid_b) = ids(2);
        let led = ledger(1000);
        led.reserve(rid_a, intent_a, vec![source(100)], 60)
            .await
            .unwrap();
        led.reserve(rid_b, intent_b, vec![source(100)], 60)
            .await
            .unwrap();

        let exec = Arc::new(FlakyExec {
            good: ExecHandle(intent_a.0),
            good_status: confirmed(),
            tracked: StdMutex::new(HashMap::new()),
        });
        let svc = ExecutionService::new(
            Arc::new(FakeSim(SimVerdict::Ok)),
            exec,
            Arc::new(FakeSettle(vec![U256::from(100u64)])),
            led,
        );
        svc.fill(pending(intent_a, rid_a)).await.unwrap();
        svc.fill(pending(intent_b, rid_b)).await.unwrap();

        let settled = svc.reconcile().await.unwrap();
        assert_eq!(
            settled,
            vec![Settled {
                intent: intent_a,
                outcome: SettledOutcome::Confirmed {
                    tx: B256::from([2; 32]),
                    block: 1,
                },
            }],
            "A's confirmed fill survives B's error"
        );
        assert_eq!(svc.pending().await.unwrap(), 1, "B stays tracked to retry");
    }
}
