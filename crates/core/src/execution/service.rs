//! The execution service: the last leg of the intent lifecycle. `fill` simulates a reserved plan
//! and, if it passes, submits it (voiding the reservation on a reject, before any nonce is spent).
//! `reconcile`, driven on a cadence, advances the tx engine and settles each fill that reached a
//! terminal state — posting the *actual* per-source amounts read from the confirmed fill (the
//! ledger returns any unfilled remainder), or voiding on failure.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::deps::execution::{Execution, SettlementReader, SimGate};
use crate::ledger::LedgerService;
use crate::obs::{info, warn};
use crate::primitives::execution::{
    ConfirmedFill, ExecHandle, ExecStatus, FillOutcome, PendingFill, SimVerdict,
};
use crate::primitives::ledger::LedgerError;
use crate::primitives::{IntentId, ReservationId, SolventError};

pub struct ExecutionService {
    sim: Arc<dyn SimGate>,
    execution: Arc<dyn Execution>,
    settlement: Arc<dyn SettlementReader>,
    ledger: Arc<LedgerService>,
    in_flight: Mutex<HashMap<IntentId, InFlight>>,
}

#[derive(Clone)]
struct InFlight {
    handle: ExecHandle,
    reservation: ReservationId,
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
            in_flight: Mutex::new(HashMap::new()),
        }
    }

    /// Simulate then privately submit a reserved plan. A simulation `Reject` voids the reservation
    /// and returns without spending a nonce; an intent already in flight returns its existing
    /// handle (idempotent — one order never fills twice).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(intent = %pending.fill_tx.intent)))]
    pub async fn fill(&self, pending: PendingFill) -> Result<FillOutcome, SolventError> {
        let intent = pending.fill_tx.intent;
        if let Some(f) = self.in_flight.lock().await.get(&intent) {
            return Ok(FillOutcome::Submitted { handle: f.handle });
        }
        match self.sim.simulate(&pending.fill_tx).await? {
            SimVerdict::Reject { reason } => {
                warn!(intent = %intent, "fill dropped by sim gate; voiding reservation");
                self.ledger.void(pending.reservation).await?;
                Ok(FillOutcome::Rejected { reason })
            }
            SimVerdict::Ok => {
                let handle = self.execution.submit(&pending.fill_tx).await?;
                self.in_flight.lock().await.insert(
                    intent,
                    InFlight {
                        handle,
                        reservation: pending.reservation,
                    },
                );
                info!(intent = %intent, "fill submitted");
                Ok(FillOutcome::Submitted { handle })
            }
        }
    }

    /// Advance the tx engine, then settle every in-flight fill that reached a terminal state: on
    /// `Confirmed`, post the actual amounts the fill pulled (read from its own settlement); on
    /// failure, void. A reservation a redelivery already settled is a no-op, so this is safe to
    /// call repeatedly.
    pub async fn reconcile(&self) -> Result<Vec<ConfirmedFill>, SolventError> {
        self.execution.tick().await?;

        let snapshot: Vec<(IntentId, InFlight)> = {
            let in_flight = self.in_flight.lock().await;
            in_flight.iter().map(|(id, f)| (*id, f.clone())).collect()
        };

        let mut settled = Vec::new();
        let mut confirmed = Vec::new();
        for (intent, f) in snapshot {
            let Some(status) = self.execution.status(f.handle).await? else {
                continue;
            };
            match status {
                ExecStatus::Confirmed { tx, .. } => {
                    match self.ledger.reservation_sources(f.reservation).await {
                        Some(sources) => {
                            let filled = self.settlement.settled(tx, &sources).await?;
                            let posted = self.ledger.post(f.reservation, &filled).await;
                            // Surface the fill only on a fresh post — a redelivery the ledger FSM
                            // rejects (`WrongState`) must not drive recapture a second time.
                            if posted.is_ok() {
                                confirmed.push(ConfirmedFill::new(intent, tx));
                            }
                            settle(posted)?;
                            info!(intent = %intent, "fill confirmed; reservation posted");
                        }
                        None => {
                            warn!(intent = %intent, "fill confirmed but reservation is gone; skipping post");
                        }
                    }
                    settled.push(intent);
                }
                ExecStatus::Failed { .. } | ExecStatus::Dropped => {
                    settle(self.ledger.void(f.reservation).await)?;
                    warn!(intent = %intent, "fill did not land; reservation voided");
                    settled.push(intent);
                }
                ExecStatus::Pending => {}
            }
        }

        if !settled.is_empty() {
            let mut in_flight = self.in_flight.lock().await;
            for intent in settled {
                in_flight.remove(&intent);
            }
        }
        Ok(confirmed)
    }

    /// Reverse a posted fill a chain reorg rolled back — the compensating ledger transition. The
    /// caller supplies the reorg signal (detecting a post-finality un-mine against the canonical
    /// chain is the reconcile worker's job, not this method's).
    pub async fn on_reorg(&self, reservation: ReservationId) -> Result<(), SolventError> {
        self.ledger.void_reorg(reservation).await
    }

    /// How many fills are still in flight — zero once every submitted fill has settled.
    pub async fn pending(&self) -> usize {
        self.in_flight.lock().await.len()
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use alloy_primitives::{Address, Bytes, B256, U256};
    use async_trait::async_trait;

    use crate::deps::execution::{ExecutionError, SettlementError, SimError};
    use crate::deps::ledger::{
        BudgetSource, BudgetSourceError, Clock, LedgerStore, LedgerStoreError,
    };
    use crate::primitives::execution::FillTx;
    use crate::primitives::ledger::{AccountKey, Reservation, ReservationSource};
    use crate::primitives::{MakerId, StrategyHash};

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
    }
    impl FakeExec {
        fn new(status: ExecStatus) -> Self {
            Self {
                status,
                submits: AtomicUsize::new(0),
            }
        }
    }
    #[async_trait]
    impl Execution for FakeExec {
        async fn submit(&self, fill: &FillTx) -> Result<ExecHandle, ExecutionError> {
            self.submits.fetch_add(1, Ordering::Relaxed);
            Ok(ExecHandle(fill.intent.0))
        }
        async fn status(&self, _: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError> {
            Ok(Some(self.status.clone()))
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
        let confirmed = svc.reconcile().await.unwrap();
        assert_eq!(
            confirmed,
            vec![ConfirmedFill::new(intent, B256::from([2; 32]))]
        );

        assert_eq!(svc.pending().await, 0);
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
        svc.reconcile().await.unwrap();

        assert_eq!(svc.pending().await, 0);
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
        assert_eq!(svc.pending().await, 0);
    }
}
