//! `ReconcileService` — one tick advances every in-flight fill to its terminal state, settles the
//! matching trade, then sweeps reservation holds whose TTL elapsed without a fill (excluding fills
//! still in flight). It owns no state; the durable execution store and the ledger do.

use std::sync::Arc;

use crate::deps::ledger::Clock;
use crate::deps::trade::{Settlement, TradeStore};
use crate::execution::ExecutionService;
use crate::ledger::LedgerService;
use crate::obs::warn;
use crate::primitives::execution::{Settled, SettledOutcome};
use crate::primitives::trade::{Trade, TradeStatus};
use crate::primitives::{IntentId, SolventError};

pub struct ReconcileService {
    execution: Arc<ExecutionService>,
    trades: Arc<dyn TradeStore>,
    ledger: Arc<LedgerService>,
    clock: Arc<dyn Clock>,
}

/// What one tick did, for the worker to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReconcileReport {
    /// Fills that reached a terminal state (confirmed or failed).
    pub settled: usize,
    /// Orphaned reservations swept past their TTL.
    pub swept: usize,
}

impl ReconcileService {
    pub fn new(
        execution: Arc<ExecutionService>,
        trades: Arc<dyn TradeStore>,
        ledger: Arc<LedgerService>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            execution,
            trades,
            ledger,
            clock,
        }
    }

    /// One reconcile pass: settle terminal fills, then sweep orphaned holds (a swept reservation's
    /// fill never landed, so its trade fails). Idempotent — the ledger and trade FSMs guard replays.
    pub async fn tick(&self) -> Result<ReconcileReport, SolventError> {
        let settled = self.execution.reconcile().await?;
        let now = self.clock.now_unix();
        for fill in &settled {
            self.settle_trade(fill, now).await?;
        }

        let in_flight = self.execution.tracked_reservations().await?;
        let swept = self.ledger.sweep_expired(&in_flight).await?;
        for intent in &swept {
            self.apply(*intent, |_| failed(now)).await?;
        }

        Ok(ReconcileReport {
            settled: settled.len(),
            swept: swept.len(),
        })
    }

    /// Settle the trade for a terminal fill: a confirmed exact-out fill delivered its signed floor,
    /// so `amount_out` is the order's minimum; a failed one ends terminal with no output.
    async fn settle_trade(&self, fill: &Settled, now: u64) -> Result<(), SolventError> {
        self.apply(fill.intent, |trade| match fill.outcome {
            SettledOutcome::Confirmed { tx, block } => Settlement {
                status: TradeStatus::Confirmed,
                amount_out: Some(trade.min_amount_out),
                tx_hash: Some(tx),
                block_number: Some(block),
                at: now,
            },
            SettledOutcome::Failed => failed(now),
        })
        .await
    }

    /// Resolve the trade for a settled order hash and apply its settlement; a missing trade is a
    /// no-op (the fill was tracked without a persisted trade — nothing to advance).
    async fn apply(
        &self,
        intent: IntentId,
        settlement: impl FnOnce(&Trade) -> Settlement,
    ) -> Result<(), SolventError> {
        let Some(trade) = self.trades.find_by_order(&intent).await? else {
            warn!(intent = %intent, "reconciled fill has no trade; skipping");
            return Ok(());
        };
        let outcome = settlement(&trade);
        self.trades.settle(&trade.id, &outcome).await?;
        Ok(())
    }
}

/// A terminal `Failed` settlement — a fill that reverted, dropped, or timed out unfilled.
fn failed(now: u64) -> Settlement {
    Settlement {
        status: TradeStatus::Failed,
        amount_out: None,
        tx_hash: None,
        block_number: None,
        at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex as StdMutex;

    use alloy_primitives::{Address, B256, U256};
    use async_trait::async_trait;
    use ulid::Ulid;

    use crate::deps::execution::{
        Execution, ExecutionError, SettlementError, SettlementReader, SimError, SimGate,
    };
    use crate::deps::ledger::{BudgetSource, BudgetSourceError, LedgerStore, LedgerStoreError};
    use crate::deps::trade::{CreateResult, Page, TradeFilter, TradeStoreError};
    use crate::primitives::execution::{ExecHandle, ExecStatus, FillTx, SimVerdict, TrackedFill};
    use crate::primitives::ledger::{AccountKey, Reservation, ReservationSource};
    use crate::primitives::trade::{TradeAttempt, TradeId, TradeInfo, TradeLeg};
    use crate::primitives::{MakerId, ReservationId, StrategyHash};

    const MAKER: u8 = 7;
    const TOKEN: u8 = 9;
    const MIN_OUT: u64 = 500;

    fn ids(n: u8) -> (IntentId, ReservationId) {
        (
            IntentId(B256::from([n; 32])),
            ReservationId(B256::from([n; 32])),
        )
    }
    fn wallet_account() -> AccountKey {
        AccountKey::WalletBudget {
            maker: MakerId(Address::from([MAKER; 20])),
            token: Address::from([TOKEN; 20]),
        }
    }
    fn source(amount: u64) -> ReservationSource {
        ReservationSource {
            maker: MakerId(Address::from([MAKER; 20])),
            strategy_hash: StrategyHash(B256::from([1; 32])),
            token: Address::from([TOKEN; 20]),
            amount: U256::from(amount),
        }
    }

    struct FakeSim;
    #[async_trait]
    impl SimGate for FakeSim {
        async fn simulate(&self, _: &FillTx) -> Result<SimVerdict, SimError> {
            Ok(SimVerdict::Ok)
        }
    }

    /// A tx engine pre-seeded with the fills it is tracking (as if recovered from durable state),
    /// reporting a fixed status for each.
    struct FakeExec {
        status: ExecStatus,
        tracked: StdMutex<HashMap<IntentId, ReservationId>>,
    }
    impl FakeExec {
        fn seeded(status: ExecStatus, fills: &[(IntentId, ReservationId)]) -> Self {
            Self {
                status,
                tracked: StdMutex::new(fills.iter().copied().collect()),
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
                .insert(fill.intent, reservation);
            Ok(ExecHandle(fill.intent.0))
        }
        async fn status(&self, handle: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError> {
            Ok(self
                .tracked
                .lock()
                .unwrap()
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
                .map(|(i, r)| TrackedFill::new(*i, *r))
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

    struct NoopLedgerStore;
    #[async_trait]
    impl LedgerStore for NoopLedgerStore {
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
            Ok(Vec::new())
        }
    }

    struct FakeBudget(u64);
    #[async_trait]
    impl BudgetSource for FakeBudget {
        async fn budget(&self, _: &AccountKey) -> Result<U256, BudgetSourceError> {
            Ok(U256::from(self.0))
        }
    }

    struct ManualClock(AtomicU64);
    impl ManualClock {
        fn set(&self, t: u64) {
            self.0.store(t, Ordering::SeqCst);
        }
    }
    impl Clock for ManualClock {
        fn now_unix(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    /// A trade store keyed by order hash — enough to seed a trade and read back its settlement.
    #[derive(Default)]
    struct MemTrades {
        rows: StdMutex<BTreeMap<B256, Trade>>,
    }
    impl MemTrades {
        fn seed(&self, trade: Trade) {
            self.rows.lock().unwrap().insert(trade.order_hash.0, trade);
        }
        fn get(&self, order_hash: IntentId) -> Trade {
            self.rows
                .lock()
                .unwrap()
                .get(&order_hash.0)
                .cloned()
                .unwrap()
        }
    }
    #[async_trait]
    impl TradeStore for MemTrades {
        async fn create(
            &self,
            _: &Trade,
            _: &[TradeLeg],
            _: &[TradeAttempt],
        ) -> Result<CreateResult, TradeStoreError> {
            unreachable!("reconcile never creates trades")
        }
        async fn advance(
            &self,
            _: &TradeId,
            _: TradeStatus,
            _: u64,
        ) -> Result<(), TradeStoreError> {
            Ok(())
        }
        async fn settle(&self, id: &TradeId, outcome: &Settlement) -> Result<(), TradeStoreError> {
            let mut rows = self.rows.lock().unwrap();
            if let Some(trade) = rows.values_mut().find(|t| t.id == *id) {
                if trade.settled_at.is_none() {
                    trade.status = outcome.status;
                    trade.amount_out = outcome.amount_out;
                    trade.tx_hash = outcome.tx_hash;
                    trade.block_number = outcome.block_number;
                    trade.settled_at = Some(outcome.at);
                }
            }
            Ok(())
        }
        async fn info(&self, _: &TradeId) -> Result<Option<TradeInfo>, TradeStoreError> {
            Ok(None)
        }
        async fn find_by_order(
            &self,
            order_hash: &IntentId,
        ) -> Result<Option<Trade>, TradeStoreError> {
            Ok(self.rows.lock().unwrap().get(&order_hash.0).cloned())
        }
        async fn list(&self, _: &TradeFilter, _: &Page) -> Result<Vec<Trade>, TradeStoreError> {
            Ok(Vec::new())
        }
    }

    fn trade(order_hash: IntentId) -> Trade {
        Trade {
            id: TradeId(Ulid::from_parts(1, u128::from(order_hash.0 .0[0]))),
            order_hash,
            taker: Address::ZERO,
            token_in: Address::from([TOKEN; 20]),
            token_out: Address::from([TOKEN; 20]),
            amount_in: U256::from(1000u64),
            min_amount_out: U256::from(MIN_OUT),
            amount_out: None,
            status: TradeStatus::Reserved,
            deadline_block: 0,
            signature: None,
            price_impact_pct: None,
            surplus: None,
            tx_hash: None,
            block_number: None,
            created_at: 0,
            settled_at: None,
        }
    }

    /// Wire a reconcile service over real ledger + execution services, with a hold already reserved
    /// for `intent`/`rid` and the exec engine seeded to report `status` for the tracked fills.
    async fn setup(
        status: ExecStatus,
        settled: Vec<U256>,
        seeded: &[(IntentId, ReservationId)],
        now: u64,
    ) -> (
        ReconcileService,
        Arc<MemTrades>,
        Arc<LedgerService>,
        Arc<ManualClock>,
    ) {
        let clock = Arc::new(ManualClock(AtomicU64::new(now)));
        let clock_dyn: Arc<dyn Clock> = clock.clone();
        let ledger = Arc::new(LedgerService::new(
            Arc::new(NoopLedgerStore),
            Arc::new(FakeBudget(1_000)),
            clock_dyn.clone(),
        ));
        let execution = Arc::new(ExecutionService::new(
            Arc::new(FakeSim),
            Arc::new(FakeExec::seeded(status, seeded)),
            Arc::new(FakeSettle(settled)),
            Arc::clone(&ledger),
        ));
        let trades = Arc::new(MemTrades::default());
        let reconcile = ReconcileService::new(
            Arc::clone(&execution),
            Arc::clone(&trades) as Arc<dyn TradeStore>,
            Arc::clone(&ledger),
            clock_dyn,
        );
        (reconcile, trades, ledger, clock)
    }

    #[tokio::test]
    async fn confirmed_fill_settles_the_trade_confirmed() {
        let (intent, rid) = ids(1);
        let tx = B256::from([2; 32]);
        let (reconcile, trades, ledger, _clock) = setup(
            ExecStatus::Confirmed { block: 42, tx },
            vec![U256::from(100u64)],
            &[(intent, rid)],
            1000,
        )
        .await;
        ledger
            .reserve(rid, intent, vec![source(100)], 60)
            .await
            .unwrap();
        trades.seed(trade(intent));

        let report = reconcile.tick().await.unwrap();
        assert_eq!(report.settled, 1);
        assert_eq!(report.swept, 0);

        let settled = trades.get(intent);
        assert_eq!(settled.status, TradeStatus::Confirmed);
        // Exact-out: the delivered output is the signed floor, not the per-source pulled input.
        assert_eq!(settled.amount_out, Some(U256::from(MIN_OUT)));
        assert_eq!(settled.tx_hash, Some(tx));
        assert_eq!(settled.block_number, Some(42));
    }

    #[tokio::test]
    async fn failed_fill_settles_the_trade_failed() {
        let (intent, rid) = ids(2);
        let (reconcile, trades, ledger, _clock) = setup(
            ExecStatus::Failed {
                reason: "revert".into(),
            },
            vec![],
            &[(intent, rid)],
            1000,
        )
        .await;
        ledger
            .reserve(rid, intent, vec![source(100)], 60)
            .await
            .unwrap();
        trades.seed(trade(intent));

        reconcile.tick().await.unwrap();
        let settled = trades.get(intent);
        assert_eq!(settled.status, TradeStatus::Failed);
        assert_eq!(settled.amount_out, None);
    }

    #[tokio::test]
    async fn swept_orphan_fails_the_trade_and_restores_the_hold() {
        let (intent, rid) = ids(3);
        // The fill was never tracked (a crash between reserve and submit) — so it is not in-flight.
        let (reconcile, trades, ledger, clock) =
            setup(ExecStatus::Pending, vec![], &[], 1000).await;
        ledger
            .reserve(rid, intent, vec![source(100)], 60)
            .await
            .unwrap();
        trades.seed(trade(intent));
        assert_eq!(ledger.available(&wallet_account()), U256::from(900u64));

        // Past the TTL, the orphaned hold is swept and its trade fails.
        clock.set(1100);
        let report = reconcile.tick().await.unwrap();
        assert_eq!(report.swept, 1);
        assert_eq!(trades.get(intent).status, TradeStatus::Failed);
        assert_eq!(ledger.available(&wallet_account()), U256::from(1000u64));
    }

    #[tokio::test]
    async fn settled_fill_without_a_trade_is_skipped() {
        let (intent, rid) = ids(4);
        let (reconcile, _trades, ledger, _clock) = setup(
            ExecStatus::Confirmed {
                block: 1,
                tx: B256::from([9; 32]),
            },
            vec![U256::from(100u64)],
            &[(intent, rid)],
            1000,
        )
        .await;
        ledger
            .reserve(rid, intent, vec![source(100)], 60)
            .await
            .unwrap();
        // No trade seeded — reconcile must not error.
        let report = reconcile.tick().await.unwrap();
        assert_eq!(report.settled, 1);
    }
}
