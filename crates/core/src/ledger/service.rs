//! The single-writer ledger service: durable-first (check → persist → commit under one
//! `tokio::Mutex`) so the in-memory ledger never leads the store, publishing a lock-free `ArcSwap`
//! of `available` for the quote path.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use alloy_primitives::U256;
use arc_swap::ArcSwap;
use tokio::sync::Mutex;

use crate::deps::ledger::{BudgetSource, Clock, LedgerStore};
use crate::primitives::ledger::{AccountKey, Ledger, Reservation, ReservationSource};
use crate::primitives::{IntentId, ReservationId, SolventError};

/// The available room at every known account, republished after each command for lock-free reads.
#[derive(Debug, Default)]
pub struct AvailableSnapshot(pub BTreeMap<AccountKey, U256>);

impl AvailableSnapshot {
    /// Available room at `account`, or zero for an account with no reservations.
    pub fn available(&self, account: &AccountKey) -> U256 {
        self.0.get(account).copied().unwrap_or(U256::ZERO)
    }
}

pub struct LedgerService {
    ledger: Mutex<Ledger>,
    store: Arc<dyn LedgerStore>,
    budgets: Arc<dyn BudgetSource>,
    clock: Arc<dyn Clock>,
    available: ArcSwap<AvailableSnapshot>,
}

impl LedgerService {
    pub fn new(
        store: Arc<dyn LedgerStore>,
        budgets: Arc<dyn BudgetSource>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            ledger: Mutex::new(Ledger::default()),
            store,
            budgets,
            clock,
            available: ArcSwap::from_pointee(AvailableSnapshot::default()),
        }
    }

    /// Admit a reservation for `ttl_secs`, holding both ceilings; durable-first under the writer lock.
    pub async fn reserve(
        &self,
        id: ReservationId,
        intent: IntentId,
        sources: Vec<ReservationSource>,
        ttl_secs: u64,
    ) -> Result<(), SolventError> {
        // Read budgets before the lock, off the critical section.
        let budgets = self.read_budgets(&accounts_of(&sources)).await?;
        let expires_at = self.clock.now_unix().saturating_add(ttl_secs);
        let reservation = Reservation::new(id, intent, sources, expires_at);

        let mut ledger = self.ledger.lock().await;
        for (account, budget) in &budgets {
            ledger.set_budget(*account, *budget);
        }
        ledger.can_reserve(&reservation)?;
        self.store.reserve(&reservation).await?;
        ledger.restore(reservation);
        self.publish(&ledger);
        Ok(())
    }

    /// Record settlement with the amount actually pulled per source.
    pub async fn post(&self, id: ReservationId, filled: &[U256]) -> Result<(), SolventError> {
        let mut ledger = self.ledger.lock().await;
        self.store.post(id, filled).await?;
        ledger.post(id, filled)?;
        self.publish(&ledger);
        Ok(())
    }

    /// Release a lost auction / reverted fill.
    pub async fn void(&self, id: ReservationId) -> Result<(), SolventError> {
        let mut ledger = self.ledger.lock().await;
        self.store.void(id).await?;
        ledger.void(id)?;
        self.publish(&ledger);
        Ok(())
    }

    /// Reverse a posted settlement a reorg rolled back.
    pub async fn void_reorg(&self, id: ReservationId) -> Result<(), SolventError> {
        let mut ledger = self.ledger.lock().await;
        self.store.void_reorg(id).await?;
        ledger.void_reorg(id)?;
        self.publish(&ledger);
        Ok(())
    }

    /// Expire every pending reservation past its TTL as of now, restoring their holds. Returns how
    /// many were swept.
    pub async fn sweep_expired(&self) -> Result<usize, SolventError> {
        let now = self.clock.now_unix();
        let mut ledger = self.ledger.lock().await;
        let expired = ledger.expired_as_of(now);
        for id in &expired {
            self.store.expire(*id).await?;
            ledger.expire(*id)?;
        }
        if !expired.is_empty() {
            self.publish(&ledger);
        }
        Ok(expired.len())
    }

    /// Rebuild the in-memory holds from the durably-open reservations after a restart; `restore` is
    /// unconditional, so a standing promise isn't re-litigated. Requires the registry synced first.
    pub async fn recover(&self) -> Result<(), SolventError> {
        let open = self.store.open_reservations().await?;
        let mut ledger = self.ledger.lock().await;
        for reservation in open {
            let budgets = self
                .read_budgets(&accounts_of(&reservation.sources))
                .await?;
            for (account, budget) in budgets {
                ledger.set_budget(account, budget);
            }
            ledger.restore(reservation);
        }
        self.publish(&ledger);
        Ok(())
    }

    /// Available room at `account`, read lock-free — the quote path's entry point.
    pub fn available(&self, account: &AccountKey) -> U256 {
        self.available.load().available(account)
    }

    async fn read_budgets(
        &self,
        accounts: &BTreeSet<AccountKey>,
    ) -> Result<Vec<(AccountKey, U256)>, SolventError> {
        let mut budgets = Vec::with_capacity(accounts.len());
        for account in accounts {
            budgets.push((*account, self.budgets.budget(account).await?));
        }
        Ok(budgets)
    }

    fn publish(&self, ledger: &Ledger) {
        self.available
            .store(Arc::new(AvailableSnapshot(ledger.available_snapshot())));
    }
}

fn accounts_of(sources: &[ReservationSource]) -> BTreeSet<AccountKey> {
    sources.iter().flat_map(|s| s.accounts()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deps::ledger::{BudgetSourceError, LedgerStoreError};
    use crate::primitives::ledger::ReservationState;
    use crate::primitives::{MakerId, StrategyHash};
    use alloy_primitives::{Address, B256};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct FakeBudget {
        budgets: BTreeMap<AccountKey, U256>,
    }
    #[async_trait]
    impl BudgetSource for FakeBudget {
        async fn budget(&self, account: &AccountKey) -> Result<U256, BudgetSourceError> {
            Ok(self.budgets.get(account).copied().unwrap_or(U256::ZERO))
        }
    }

    struct ManualClock(AtomicU64);
    impl ManualClock {
        fn new(t: u64) -> Self {
            Self(AtomicU64::new(t))
        }
        fn set(&self, t: u64) {
            self.0.store(t, Ordering::SeqCst);
        }
    }
    impl Clock for ManualClock {
        fn now_unix(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    // An in-memory `LedgerStore` mirror: the durable side of the service, in a map.
    type Row = (Reservation, ReservationState, Vec<U256>);
    #[derive(Default)]
    struct FakeStore {
        rows: StdMutex<BTreeMap<ReservationId, Row>>,
    }
    impl FakeStore {
        fn set_state(&self, id: ReservationId, from: ReservationState, to: ReservationState) {
            if let Some(row) = self.rows.lock().unwrap().get_mut(&id) {
                if row.1 == from {
                    row.1 = to;
                }
            }
        }
    }
    #[async_trait]
    impl LedgerStore for FakeStore {
        async fn reserve(&self, r: &Reservation) -> Result<(), LedgerStoreError> {
            self.rows
                .lock()
                .unwrap()
                .entry(r.id)
                .or_insert_with(|| (r.clone(), ReservationState::Pending, Vec::new()));
            Ok(())
        }
        async fn post(&self, id: ReservationId, filled: &[U256]) -> Result<(), LedgerStoreError> {
            if let Some(row) = self.rows.lock().unwrap().get_mut(&id) {
                if row.1 == ReservationState::Pending {
                    row.1 = ReservationState::Posted;
                    row.2 = filled.to_vec();
                }
            }
            Ok(())
        }
        async fn void(&self, id: ReservationId) -> Result<(), LedgerStoreError> {
            self.set_state(id, ReservationState::Pending, ReservationState::Voided);
            Ok(())
        }
        async fn expire(&self, id: ReservationId) -> Result<(), LedgerStoreError> {
            self.set_state(id, ReservationState::Pending, ReservationState::Expired);
            Ok(())
        }
        async fn void_reorg(&self, id: ReservationId) -> Result<(), LedgerStoreError> {
            self.set_state(id, ReservationState::Posted, ReservationState::ReorgOpen);
            Ok(())
        }
        async fn open_reservations(&self) -> Result<Vec<Reservation>, LedgerStoreError> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .values()
                .filter(|(_, state, _)| *state == ReservationState::Pending)
                .map(|(r, _, _)| r.clone())
                .collect())
        }
    }

    fn maker(n: u8) -> MakerId {
        MakerId(Address::from([n; 20]))
    }
    fn strat(n: u8) -> StrategyHash {
        StrategyHash(B256::from([n; 32]))
    }
    fn token(n: u8) -> Address {
        Address::from([n; 20])
    }
    fn resv(n: u8) -> ReservationId {
        ReservationId(B256::from([n; 32]))
    }
    fn intent(n: u8) -> IntentId {
        IntentId(B256::from([n; 32]))
    }
    fn wallet(m: u8, t: u8) -> AccountKey {
        AccountKey::WalletBudget {
            maker: maker(m),
            token: token(t),
        }
    }
    fn virt(m: u8, s: u8, t: u8) -> AccountKey {
        AccountKey::StrategyVirtual {
            maker: maker(m),
            strategy_hash: strat(s),
            token: token(t),
        }
    }
    fn src(m: u8, s: u8, t: u8, amount: u64) -> ReservationSource {
        ReservationSource {
            maker: maker(m),
            strategy_hash: strat(s),
            token: token(t),
            amount: U256::from(amount),
        }
    }
    fn amt(n: u64) -> U256 {
        U256::from(n)
    }

    fn build(
        budgets: &[(AccountKey, u64)],
        now: u64,
    ) -> (LedgerService, Arc<FakeStore>, Arc<ManualClock>) {
        let store = Arc::new(FakeStore::default());
        let clock = Arc::new(ManualClock::new(now));
        let budgets = Arc::new(FakeBudget {
            budgets: budgets.iter().map(|(a, v)| (*a, amt(*v))).collect(),
        });
        let service = LedgerService::new(store.clone(), budgets, clock.clone());
        (service, store, clock)
    }

    #[tokio::test]
    async fn reserve_holds_persists_and_publishes_available() {
        let (svc, store, _) = build(&[(wallet(1, 3), 1_000_000), (virt(1, 1, 3), 600_000)], 1000);
        svc.reserve(resv(1), intent(1), vec![src(1, 1, 3, 600_000)], 60)
            .await
            .unwrap();
        assert_eq!(svc.available(&wallet(1, 3)), amt(400_000));
        assert_eq!(svc.available(&virt(1, 1, 3)), U256::ZERO);
        assert_eq!(store.open_reservations().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn alice_race_admits_exactly_one() {
        let (svc, _, _) = build(
            &[
                (wallet(1, 3), 1_000_000),
                (virt(1, 1, 3), 600_000),
                (virt(1, 2, 3), 600_000),
            ],
            1000,
        );
        let a = svc.reserve(resv(1), intent(1), vec![src(1, 1, 3, 600_000)], 60);
        let b = svc.reserve(resv(2), intent(2), vec![src(1, 2, 3, 600_000)], 60);
        let (ra, rb) = tokio::join!(a, b);
        assert_eq!(
            [ra.is_ok(), rb.is_ok()]
                .into_iter()
                .filter(|ok| *ok)
                .count(),
            1,
            "the shared wallet admits exactly one"
        );
        assert_eq!(svc.available(&wallet(1, 3)), amt(400_000));
    }

    #[tokio::test]
    async fn post_consumes_and_void_restores() {
        let (svc, _, _) = build(&[(wallet(1, 3), 1_000_000), (virt(1, 1, 3), 600_000)], 1000);
        svc.reserve(resv(1), intent(1), vec![src(1, 1, 3, 600_000)], 60)
            .await
            .unwrap();
        svc.post(resv(1), &[amt(600_000)]).await.unwrap();
        assert_eq!(svc.available(&wallet(1, 3)), amt(400_000));

        let (svc2, _, _) = build(&[(wallet(1, 3), 1_000_000), (virt(1, 1, 3), 600_000)], 1000);
        svc2.reserve(resv(2), intent(2), vec![src(1, 1, 3, 600_000)], 60)
            .await
            .unwrap();
        svc2.void(resv(2)).await.unwrap();
        assert_eq!(svc2.available(&wallet(1, 3)), amt(1_000_000));
    }

    #[tokio::test]
    async fn sweep_expires_elapsed_and_restores() {
        let (svc, store, clock) =
            build(&[(wallet(1, 3), 1_000_000), (virt(1, 1, 3), 600_000)], 1000);
        svc.reserve(resv(1), intent(1), vec![src(1, 1, 3, 600_000)], 60)
            .await
            .unwrap();
        // At t=1000 the reservation (expires_at 1060) is not yet due.
        assert_eq!(svc.sweep_expired().await.unwrap(), 0);
        assert_eq!(svc.available(&wallet(1, 3)), amt(400_000));

        clock.set(1100);
        assert_eq!(svc.sweep_expired().await.unwrap(), 1);
        assert_eq!(svc.available(&wallet(1, 3)), amt(1_000_000));
        assert!(store.open_reservations().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn recover_rebuilds_holds_from_the_store() {
        let (svc, store, _) = build(&[(wallet(1, 3), 1_000_000), (virt(1, 1, 3), 600_000)], 1000);
        svc.reserve(resv(1), intent(1), vec![src(1, 1, 3, 600_000)], 60)
            .await
            .unwrap();

        // A fresh service over the same store — a simulated restart — starts empty, then rebuilds.
        let budgets = Arc::new(FakeBudget {
            budgets: [
                (wallet(1, 3), amt(1_000_000)),
                (virt(1, 1, 3), amt(600_000)),
            ]
            .into_iter()
            .collect(),
        });
        let restarted =
            LedgerService::new(store.clone(), budgets, Arc::new(ManualClock::new(1000)));
        assert_eq!(restarted.available(&wallet(1, 3)), U256::ZERO);
        restarted.recover().await.unwrap();
        assert_eq!(restarted.available(&wallet(1, 3)), amt(400_000));
    }
}
