//! The single-writer ledger service: durable-first (check → persist → commit under one
//! `tokio::Mutex`) so the in-memory ledger never leads the store, publishing a lock-free `ArcSwap`
//! of `available` for the quote path.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use alloy_primitives::U256;
use arc_swap::ArcSwap;
use tokio::sync::Mutex;

use crate::deps::ledger::{BudgetSource, Clock, LedgerStore};
use crate::primitives::ledger::{
    AccountKey, Ledger, Reservation, ReservationSource, ReservationState,
};
use crate::primitives::registry::Snapshot;
use crate::primitives::{IntentId, RebateBatchId, ReservationId, SolventError};

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
        let reservation = Reservation::for_swap(id, intent, sources, expires_at);

        self.reserve_owned(reservation, budgets).await
    }

    /// Admit a restoration reservation. Its lifecycle is managed with the rebate batch so a
    /// generic swap sweep cannot release inventory while the strategy remains guarded.
    pub async fn reserve_rebate(
        &self,
        id: ReservationId,
        batch: RebateBatchId,
        sources: Vec<ReservationSource>,
        ttl_secs: u64,
    ) -> Result<(), SolventError> {
        let budgets = self.read_budgets(&accounts_of(&sources)).await?;
        let expires_at = self.clock.now_unix().saturating_add(ttl_secs);
        let reservation = Reservation::for_rebate(id, batch, sources, expires_at);

        self.reserve_owned(reservation, budgets).await
    }

    async fn reserve_owned(
        &self,
        reservation: Reservation,
        budgets: Vec<(AccountKey, U256)>,
    ) -> Result<(), SolventError> {
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

    /// Release one elapsed reservation chosen by its owning workflow.
    pub async fn expire(&self, id: ReservationId) -> Result<(), SolventError> {
        let mut ledger = self.ledger.lock().await;
        self.store.expire(id).await?;
        ledger.expire(id)?;
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

    /// The sources of a pending `id`, or `None` once it is absent or terminal.
    pub async fn reservation_sources(&self, id: ReservationId) -> Option<Vec<ReservationSource>> {
        self.ledger
            .lock()
            .await
            .reservation(&id)
            .filter(|reservation| reservation.state == ReservationState::Pending)
            .map(|r| r.sources.clone())
    }

    /// Expire every pending reservation past its TTL as of now — except those in `exclude` (fills
    /// still in flight, whose holds must not be released while their tx can still land) — restoring
    /// the swept holds. Returns the intents of the swept reservations, so their trades can settle.
    pub async fn sweep_expired(
        &self,
        exclude: &BTreeSet<ReservationId>,
    ) -> Result<Vec<IntentId>, SolventError> {
        let now = self.clock.now_unix();
        let mut ledger = self.ledger.lock().await;
        let expired: Vec<ReservationId> = ledger
            .expired_swaps_as_of(now)
            .into_iter()
            .filter(|id| !exclude.contains(id))
            .collect();
        let mut swept = Vec::with_capacity(expired.len());
        for id in &expired {
            let intent = ledger.reservation(id).and_then(|r| r.owner.swap_intent());
            self.store.expire(*id).await?;
            ledger.expire(*id)?;
            swept.extend(intent);
        }
        if !expired.is_empty() {
            self.publish(&ledger);
        }
        Ok(swept)
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

    /// The whole caps snapshot (every active account net of holds), read lock-free — the one
    /// snapshot the depth curve and the router both read, with no per-request RPC.
    pub fn snapshot(&self) -> Arc<AvailableSnapshot> {
        self.available.load_full()
    }

    /// Refresh the full book's budgets off the request path: one batched read of every active
    /// maker's caps, replacing the fed budgets wholesale (holds are preserved) and republishing.
    /// A supervised poller drives this; a failed tick keeps the last-good snapshot and retries.
    pub async fn sync_budgets(&self, registry: &Snapshot) -> Result<(), SolventError> {
        let budgets = self.budgets.budgets(&active_accounts(registry)).await?;
        let mut ledger = self.ledger.lock().await;
        ledger.set_budgets(budgets);
        self.publish(&ledger);
        Ok(())
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

/// The accounts to sync: each active strategy's wallet and virtual on both of its pair's tokens.
/// Wallets dedup per `(maker, token)` — a maker's strategies share one wallet.
fn active_accounts(snapshot: &Snapshot) -> Vec<AccountKey> {
    let mut walleted: BTreeSet<AccountKey> = BTreeSet::new();
    let mut accounts: Vec<AccountKey> = Vec::new();
    for strategy in snapshot.active_strategies() {
        let Some(pair) = strategy.pair() else {
            continue;
        };
        for token in [pair.lo, pair.hi] {
            let wallet = AccountKey::WalletBudget {
                maker: strategy.key.maker,
                token,
            };
            if walleted.insert(wallet) {
                accounts.push(wallet);
            }
            accounts.push(AccountKey::StrategyVirtual {
                maker: strategy.key.maker,
                strategy_hash: strategy.key.strategy_hash,
                token,
            });
        }
    }
    accounts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deps::ledger::{BudgetSourceError, LedgerStoreError};
    use crate::primitives::ledger::{LedgerError, ReservationState};
    use crate::primitives::{MakerId, RebateBatchId, StrategyHash};
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
        assert!(svc
            .sweep_expired(&BTreeSet::new())
            .await
            .unwrap()
            .is_empty());
        assert_eq!(svc.available(&wallet(1, 3)), amt(400_000));

        // An in-flight reservation is excluded even once its TTL elapses.
        clock.set(1100);
        assert!(svc
            .sweep_expired(&BTreeSet::from([resv(1)]))
            .await
            .unwrap()
            .is_empty());
        assert_eq!(svc.available(&wallet(1, 3)), amt(400_000), "still held");

        // Not excluded: swept, its hold restored, and its intent reported.
        assert_eq!(
            svc.sweep_expired(&BTreeSet::new()).await.unwrap(),
            vec![intent(1)]
        );
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

    #[tokio::test]
    async fn sync_budgets_publishes_the_whole_book_net_of_holds() {
        use crate::primitives::registry::{Curve, CurveSpec, MakerStrategy, Snapshot, StrategyKey};

        // One active strategy for maker 1 over tokens (3, 4) — its virtuals come from the registry.
        let key = StrategyKey {
            maker: maker(1),
            app: Address::ZERO,
            strategy_hash: strat(1),
        };
        let mut strategy = MakerStrategy::new(key, &[]);
        strategy.curve = CurveSpec::Priceable {
            curve: Curve::Xyc,
            fees_in_bps: vec![],
        };
        strategy.balances.insert(token(3), amt(600_000));
        strategy.balances.insert(token(4), amt(600_000));
        let registry = Snapshot::from_strategies([strategy]);

        let (svc, _, _) = build(
            &[
                (wallet(1, 3), 1_000_000),
                (virt(1, 1, 3), 600_000),
                (wallet(1, 4), 1_000_000),
                (virt(1, 1, 4), 600_000),
            ],
            1000,
        );

        // The whole book is published from the sync alone — no reservation needed to see caps.
        svc.sync_budgets(&registry).await.unwrap();
        assert_eq!(svc.available(&wallet(1, 3)), amt(1_000_000));
        assert_eq!(svc.available(&virt(1, 1, 3)), amt(600_000));

        // A reservation nets its hold out of the same snapshot.
        svc.reserve(resv(1), intent(1), vec![src(1, 1, 3, 400_000)], 60)
            .await
            .unwrap();
        assert_eq!(svc.available(&wallet(1, 3)), amt(600_000));
        assert_eq!(svc.available(&virt(1, 1, 3)), amt(200_000));

        // Re-syncing replaces budgets wholesale but preserves the standing hold.
        svc.sync_budgets(&registry).await.unwrap();
        assert_eq!(svc.snapshot().available(&wallet(1, 3)), amt(600_000));
        assert_eq!(svc.snapshot().available(&virt(1, 1, 3)), amt(200_000));
    }

    #[tokio::test]
    async fn swap_and_rebate_reservations_share_both_inventory_ceilings() {
        let (svc, _, _) = build(
            &[
                (wallet(1, 3), 100),
                (virt(1, 1, 3), 80),
                (virt(1, 2, 3), 80),
            ],
            1_000,
        );
        svc.reserve(resv(1), intent(1), vec![src(1, 1, 3, 50)], 60)
            .await
            .unwrap();

        let exceeds_strategy = svc
            .reserve_rebate(
                resv(2),
                RebateBatchId(B256::from([2; 32])),
                vec![src(1, 1, 3, 31)],
                60,
            )
            .await;
        assert!(matches!(
            exceeds_strategy,
            Err(SolventError::Ledger(LedgerError::Insufficient(_)))
        ));

        let exceeds_wallet = svc
            .reserve_rebate(
                resv(2),
                RebateBatchId(B256::from([2; 32])),
                vec![src(1, 2, 3, 51)],
                60,
            )
            .await;
        assert!(matches!(
            exceeds_wallet,
            Err(SolventError::Ledger(LedgerError::Insufficient(_)))
        ));

        svc.reserve_rebate(
            resv(2),
            RebateBatchId(B256::from([2; 32])),
            vec![src(1, 2, 3, 50)],
            0,
        )
        .await
        .unwrap();
        assert_eq!(svc.available(&wallet(1, 3)), U256::ZERO);
        assert_eq!(svc.available(&virt(1, 1, 3)), amt(30));
        assert_eq!(svc.available(&virt(1, 2, 3)), amt(30));

        assert!(svc
            .sweep_expired(&BTreeSet::new())
            .await
            .unwrap()
            .is_empty());
        assert_eq!(svc.available(&wallet(1, 3)), U256::ZERO);

        svc.void(resv(2)).await.unwrap();
        assert_eq!(svc.available(&wallet(1, 3)), amt(50));
        assert_eq!(svc.available(&virt(1, 2, 3)), amt(80));
    }
}
