//! The pure double-entry reservation engine: a zero-I/O state machine making an over-committable
//! maker balance safe to promise across concurrent intents. Each source is held at both ceilings
//! (shared wallet + strategy virtual); budget is fed in, so it owns `pending`/`consumed` and derives
//! `available`.

use std::collections::{BTreeMap, BTreeSet};

use alloy_primitives::U256;
use thiserror::Error;

use crate::primitives::ReservationId;

use super::account::AccountKey;
use super::reservation::{Reservation, ReservationSource, ReservationState};

/// One account's two held compartments; `available` is derived from these and the fed budget.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Held {
    pending: U256,
    consumed: U256,
}

/// The pure ledger: fed budgets, the holds derived from reservations, and the fills recorded at
/// `post` so a later reorg can reverse them.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    budgets: BTreeMap<AccountKey, U256>,
    held: BTreeMap<AccountKey, Held>,
    reservations: BTreeMap<ReservationId, Reservation>,
    filled: BTreeMap<ReservationId, Vec<U256>>,
}

/// Why a ledger command was refused. A pure engine error (not a `SolventError` variant until a
/// public caller branches on it, per YAGNI), mirroring the registry's `CurveError`/`PriceError`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum LedgerError {
    #[error("reservation {0} already exists")]
    Duplicate(ReservationId),
    #[error("unknown reservation {0}")]
    Unknown(ReservationId),
    #[error("reservation {id} is {found:?}, expected {expected:?}")]
    WrongState {
        id: ReservationId,
        found: ReservationState,
        expected: ReservationState,
    },
    /// Boxed: an `AccountKey` plus two `U256`s would make every `Result` oversized.
    #[error(transparent)]
    Insufficient(Box<Shortfall>),
    #[error("{got} fill amounts for a {want}-source reservation")]
    FillCountMismatch { got: usize, want: usize },
    #[error("fill {filled} exceeds reserved {reserved} at source {index}")]
    FillExceedsReserved {
        index: usize,
        filled: U256,
        reserved: U256,
    },
}

/// The account a reserve could not fit and by how much — the payload of
/// [`LedgerError::Insufficient`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("insufficient capacity at {account:?}: need {requested}, have {available}")]
pub struct Shortfall {
    pub account: AccountKey,
    pub requested: U256,
    pub available: U256,
}

impl Ledger {
    /// Set an account's budget — the settleable cap fed from outside (chain balance/allowance for a
    /// wallet, the registry virtual for a strategy). Latest value wins; a maker's budget may drop
    /// externally, which is why `available` saturates rather than assuming `pending ≤ budget`.
    pub fn set_budget(&mut self, account: AccountKey, budget: U256) {
        self.budgets.insert(account, budget);
    }

    /// Replace the fed budgets wholesale — the periodic full-book sync. Holds are untouched, so an
    /// account that drops out of the book loses its budget (its `available` falls to zero) while any
    /// standing hold on it stays recorded.
    pub fn set_budgets(&mut self, budgets: BTreeMap<AccountKey, U256>) {
        self.budgets = budgets;
    }

    /// Admit a reservation, holding each source at both ceilings — atomically. `can_reserve` then
    /// `restore`; on any shortfall nothing is held. The durable-first service splits these across a
    /// persist, so a crash mid-way leaves the durable record as the single source of truth.
    pub fn reserve(&mut self, reservation: Reservation) -> Result<(), LedgerError> {
        self.can_reserve(&reservation)?;
        self.restore(reservation);
        Ok(())
    }

    /// Test the two-ceiling admission without mutating: rejects a duplicate id or any account short
    /// of the aggregate demand (sources drawing the same wallet token sum).
    pub fn can_reserve(&self, reservation: &Reservation) -> Result<(), LedgerError> {
        if self.reservations.contains_key(&reservation.id) {
            return Err(LedgerError::Duplicate(reservation.id));
        }
        for (account, &requested) in &Self::aggregate_demand(&reservation.sources) {
            let available = self.available(account);
            if available < requested {
                return Err(LedgerError::Insufficient(Box::new(Shortfall {
                    account: *account,
                    requested,
                    available,
                })));
            }
        }
        Ok(())
    }

    /// Place a reservation's holds and record it `Pending`, without re-checking admission — the
    /// commit half of an already-passed `can_reserve`, and the primitive recovery replays to rebuild
    /// holds from the durable record (a promise stands even if the live budget has since dropped).
    /// Idempotent on the id: a reservation already held is left untouched, so replaying recovery
    /// never double-counts.
    pub fn restore(&mut self, reservation: Reservation) {
        if self.reservations.contains_key(&reservation.id) {
            return;
        }
        for (account, requested) in Self::aggregate_demand(&reservation.sources) {
            let held = self.held.entry(account).or_default();
            held.pending = held.pending.saturating_add(requested);
        }
        self.reservations.insert(reservation.id, reservation);
    }

    /// Protect a prepared hold from TTL expiry while retaining it in the pending compartment.
    pub fn commit(&mut self, id: ReservationId) -> Result<(), LedgerError> {
        let reservation = self.reservations.get(&id).ok_or(LedgerError::Unknown(id))?;
        match reservation.state {
            ReservationState::Pending => {
                self.transition(id, ReservationState::Committed);
                Ok(())
            }
            ReservationState::Committed => Ok(()),
            state => Err(LedgerError::WrongState {
                id,
                found: state,
                expected: ReservationState::Pending,
            }),
        }
    }

    /// Settle a pending reservation with the amount actually pulled per source (`filled[i] ≤
    /// sources[i].amount`): each hold moves `pending → consumed` and any unfilled remainder returns
    /// to available. The fills are recorded so a reorg can reverse exactly what was consumed.
    pub fn post(&mut self, id: ReservationId, filled: &[U256]) -> Result<(), LedgerError> {
        let sources = self.active_sources(id)?;
        if filled.len() != sources.len() {
            return Err(LedgerError::FillCountMismatch {
                got: filled.len(),
                want: sources.len(),
            });
        }
        for (source, (index, &fill)) in sources.iter().zip(filled.iter().enumerate()) {
            if fill > source.amount {
                return Err(LedgerError::FillExceedsReserved {
                    index,
                    filled: fill,
                    reserved: source.amount,
                });
            }
        }

        for (source, &fill) in sources.iter().zip(filled) {
            for account in source.accounts() {
                let held = self.held.entry(account).or_default();
                held.pending = held.pending.saturating_sub(source.amount);
                held.consumed = held.consumed.saturating_add(fill);
            }
        }
        self.filled.insert(id, filled.to_vec());
        self.transition(id, ReservationState::Posted);
        Ok(())
    }

    /// Release a lost auction / reverted fill: the pending holds return to available, ending
    /// `Voided`.
    pub fn void(&mut self, id: ReservationId) -> Result<(), LedgerError> {
        self.release_active(id, ReservationState::Voided)
    }

    /// Release an elapsed reservation: the pending holds return to available, ending `Expired`. The
    /// TTL sweep that decides *which* to expire lives in the service; this is the transition.
    pub fn expire(&mut self, id: ReservationId) -> Result<(), LedgerError> {
        let sources = self.pending_sources(id)?;
        self.release_sources(id, &sources, ReservationState::Expired);
        Ok(())
    }

    /// Reverse a posted settlement that a chain reorg rolled back: the consumed capital returns to
    /// available and the reservation becomes `ReorgOpen` for reconcile to re-decide — the
    /// compensating half of `post`.
    pub fn void_reorg(&mut self, id: ReservationId) -> Result<(), LedgerError> {
        let reservation = self.reservations.get(&id).ok_or(LedgerError::Unknown(id))?;
        Self::require_state(reservation, ReservationState::Posted)?;
        let sources = reservation.sources.clone();
        let filled = self.filled.remove(&id).unwrap_or_default();
        for (index, source) in sources.iter().enumerate() {
            let fill = filled.get(index).copied().unwrap_or(U256::ZERO);
            for account in source.accounts() {
                let held = self.held.entry(account).or_default();
                held.consumed = held.consumed.saturating_sub(fill);
            }
        }
        self.transition(id, ReservationState::ReorgOpen);
        Ok(())
    }

    /// Room left at an account: `budget − (pending + consumed)`, clamped at zero. A maker's budget
    /// can drop externally below its holds, so this saturates rather than underflowing.
    pub fn available(&self, account: &AccountKey) -> U256 {
        let held = self
            .held
            .get(account)
            .map_or(U256::ZERO, |h| h.pending.saturating_add(h.consumed));
        self.budget(account).saturating_sub(held)
    }

    /// An account's fed budget (zero if never funded).
    pub fn budget(&self, account: &AccountKey) -> U256 {
        self.budgets.get(account).copied().unwrap_or(U256::ZERO)
    }

    /// Amount currently held by pending reservations at `account`.
    pub fn pending(&self, account: &AccountKey) -> U256 {
        self.held.get(account).map_or(U256::ZERO, |h| h.pending)
    }

    /// Amount consumed by posted reservations at `account`.
    pub fn consumed(&self, account: &AccountKey) -> U256 {
        self.held.get(account).map_or(U256::ZERO, |h| h.consumed)
    }

    /// The reservation for `id`, in whatever state.
    pub fn reservation(&self, id: &ReservationId) -> Option<&Reservation> {
        self.reservations.get(id)
    }

    /// Every account the ledger knows mapped to its current available room — the lock-free snapshot
    /// the service republishes for the quote path after each command.
    pub fn available_snapshot(&self) -> BTreeMap<AccountKey, U256> {
        self.budgets
            .keys()
            .chain(self.held.keys())
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|account| {
                let available = self.available(&account);
                (account, available)
            })
            .collect()
    }

    /// The pending reservations whose TTL has elapsed as of `now` (unix seconds) — the sweep expires
    /// exactly these.
    pub fn expired_as_of(&self, now: u64) -> Vec<ReservationId> {
        self.reservations
            .values()
            .filter(|r| r.state == ReservationState::Pending && r.expires_at <= now)
            .map(|r| r.id)
            .collect()
    }

    /// Sum a source set's demand per account (sources on the same wallet token, or the same strategy
    /// token, add up).
    fn aggregate_demand(sources: &[ReservationSource]) -> BTreeMap<AccountKey, U256> {
        let mut demand = BTreeMap::new();
        for source in sources {
            for account in source.accounts() {
                let sum = demand.entry(account).or_insert(U256::ZERO);
                *sum = sum.saturating_add(source.amount);
            }
        }
        demand
    }

    /// Return a pending reservation's sources (cloned so the holds can be mutated), or the reason it
    /// is not reservable.
    fn pending_sources(&self, id: ReservationId) -> Result<Vec<ReservationSource>, LedgerError> {
        let reservation = self.reservations.get(&id).ok_or(LedgerError::Unknown(id))?;
        Self::require_state(reservation, ReservationState::Pending)?;
        Ok(reservation.sources.clone())
    }

    fn active_sources(&self, id: ReservationId) -> Result<Vec<ReservationSource>, LedgerError> {
        let reservation = self.reservations.get(&id).ok_or(LedgerError::Unknown(id))?;
        match reservation.state {
            ReservationState::Pending | ReservationState::Committed => {
                Ok(reservation.sources.clone())
            }
            state => Err(LedgerError::WrongState {
                id,
                found: state,
                expected: ReservationState::Pending,
            }),
        }
    }

    fn release_active(
        &mut self,
        id: ReservationId,
        terminal: ReservationState,
    ) -> Result<(), LedgerError> {
        let sources = self.active_sources(id)?;
        self.release_sources(id, &sources, terminal);
        Ok(())
    }

    fn release_sources(
        &mut self,
        id: ReservationId,
        sources: &[ReservationSource],
        terminal: ReservationState,
    ) {
        for source in sources {
            for account in source.accounts() {
                let held = self.held.entry(account).or_default();
                held.pending = held.pending.saturating_sub(source.amount);
            }
        }
        self.transition(id, terminal);
    }

    fn require_state(
        reservation: &Reservation,
        expected: ReservationState,
    ) -> Result<(), LedgerError> {
        if reservation.state == expected {
            Ok(())
        } else {
            Err(LedgerError::WrongState {
                id: reservation.id,
                found: reservation.state,
                expected,
            })
        }
    }

    /// Set a known-present reservation's state. The caller has already resolved `id`, so a missing
    /// entry is impossible and silently ignored rather than panicking.
    fn transition(&mut self, id: ReservationId, state: ReservationState) {
        if let Some(reservation) = self.reservations.get_mut(&id) {
            reservation.state = state;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{IntentId, MakerId, StrategyHash};
    use alloy_primitives::{Address, B256};
    use proptest::prelude::*;

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
    fn source(m: u8, s: u8, t: u8, amount: u64) -> ReservationSource {
        ReservationSource {
            maker: maker(m),
            strategy_hash: strat(s),
            token: token(t),
            amount: U256::from(amount),
        }
    }
    fn reservation(id: u8, sources: Vec<ReservationSource>) -> Reservation {
        Reservation::new(resv(id), IntentId(B256::from([id; 32])), sources, 0)
    }
    fn amt(n: u64) -> U256 {
        U256::from(n)
    }

    // Maker 1 sells token 3 from two strategies S1, S2; wallet budget 1M, each strategy virtual 600k.
    fn funded() -> Ledger {
        let mut l = Ledger::default();
        l.set_budget(wallet(1, 3), amt(1_000_000));
        l.set_budget(virt(1, 1, 3), amt(600_000));
        l.set_budget(virt(1, 2, 3), amt(600_000));
        l
    }

    // The shared wallet binds across strategies: two 600k reserves from *different* strategies
    // cannot both be promised against a 1M wallet, even though each strategy virtual is fundable.
    #[test]
    fn shared_wallet_admits_exactly_one() {
        let mut l = funded();
        l.reserve(reservation(1, vec![source(1, 1, 3, 600_000)]))
            .unwrap();
        let err = l
            .reserve(reservation(2, vec![source(1, 2, 3, 600_000)]))
            .unwrap_err();
        assert!(matches!(err, LedgerError::Insufficient(_)));
        assert_eq!(l.available(&wallet(1, 3)), amt(400_000));
    }

    // The second ceiling exists for the same-strategy race: the wallet still has room, but the
    // strategy virtual does not.
    #[test]
    fn strategy_virtual_binds_same_strategy_race() {
        let mut l = funded();
        l.reserve(reservation(1, vec![source(1, 1, 3, 400_000)]))
            .unwrap();
        let err = l
            .reserve(reservation(2, vec![source(1, 1, 3, 400_000)]))
            .unwrap_err();
        match err {
            LedgerError::Insufficient(shortfall) => assert_eq!(shortfall.account, virt(1, 1, 3)),
            other => panic!("expected strategy-virtual shortfall, got {other:?}"),
        }
    }

    // One infeasible source rejects the whole reservation and leaves no partial holds behind.
    #[test]
    fn multi_source_admission_is_atomic() {
        let mut l = funded();
        let err = l
            .reserve(reservation(
                1,
                vec![source(1, 1, 3, 300_000), source(1, 2, 3, 700_000)],
            ))
            .unwrap_err();
        assert!(matches!(err, LedgerError::Insufficient(_)));
        assert_eq!(l.pending(&wallet(1, 3)), U256::ZERO);
        assert_eq!(l.pending(&virt(1, 1, 3)), U256::ZERO);
        assert!(l.reservation(&resv(1)).is_none());
    }

    #[test]
    fn post_full_consumes_the_hold() {
        let mut l = funded();
        l.reserve(reservation(1, vec![source(1, 1, 3, 600_000)]))
            .unwrap();
        l.post(resv(1), &[amt(600_000)]).unwrap();
        assert_eq!(l.pending(&wallet(1, 3)), U256::ZERO);
        assert_eq!(l.consumed(&wallet(1, 3)), amt(600_000));
        assert_eq!(l.available(&wallet(1, 3)), amt(400_000));
        assert_eq!(
            l.reservation(&resv(1)).unwrap().state,
            ReservationState::Posted
        );
    }

    #[test]
    fn post_partial_restores_the_remainder() {
        let mut l = funded();
        l.reserve(reservation(1, vec![source(1, 1, 3, 600_000)]))
            .unwrap();
        l.post(resv(1), &[amt(250_000)]).unwrap();
        assert_eq!(l.consumed(&wallet(1, 3)), amt(250_000));
        assert_eq!(l.pending(&wallet(1, 3)), U256::ZERO);
        // 350k of the reserved 600k was never pulled and is spendable again.
        assert_eq!(l.available(&wallet(1, 3)), amt(750_000));
        assert_eq!(l.available(&virt(1, 1, 3)), amt(350_000));
    }

    #[test]
    fn void_and_expire_return_holds_to_available() {
        for terminal in [ReservationState::Voided, ReservationState::Expired] {
            let mut l = funded();
            l.reserve(reservation(1, vec![source(1, 1, 3, 600_000)]))
                .unwrap();
            match terminal {
                ReservationState::Voided => l.void(resv(1)).unwrap(),
                _ => l.expire(resv(1)).unwrap(),
            }
            assert_eq!(l.available(&wallet(1, 3)), amt(1_000_000));
            assert_eq!(l.pending(&wallet(1, 3)), U256::ZERO);
            assert_eq!(l.reservation(&resv(1)).unwrap().state, terminal);
        }
    }

    #[test]
    fn committed_reservation_survives_expiry_and_can_post() {
        let mut ledger = funded();
        ledger
            .reserve(reservation(1, vec![source(1, 1, 3, 100_000)]))
            .unwrap();
        ledger.commit(resv(1)).unwrap();
        assert!(ledger.expired_as_of(u64::MAX).is_empty());
        ledger.post(resv(1), &[amt(100_000)]).unwrap();
        assert_eq!(
            ledger.reservation(&resv(1)).unwrap().state,
            ReservationState::Posted
        );
    }

    #[test]
    fn void_reorg_reverses_a_posted_consumption() {
        let mut l = funded();
        l.reserve(reservation(1, vec![source(1, 1, 3, 600_000)]))
            .unwrap();
        l.post(resv(1), &[amt(600_000)]).unwrap();
        l.void_reorg(resv(1)).unwrap();
        assert_eq!(l.consumed(&wallet(1, 3)), U256::ZERO);
        assert_eq!(l.available(&wallet(1, 3)), amt(1_000_000));
        assert_eq!(
            l.reservation(&resv(1)).unwrap().state,
            ReservationState::ReorgOpen
        );
    }

    #[test]
    fn rejects_duplicate_unknown_and_wrong_state() {
        let mut l = funded();
        l.reserve(reservation(1, vec![source(1, 1, 3, 100_000)]))
            .unwrap();
        assert!(matches!(
            l.reserve(reservation(1, vec![source(1, 1, 3, 1)]))
                .unwrap_err(),
            LedgerError::Duplicate(_)
        ));
        assert!(matches!(
            l.post(resv(9), &[]).unwrap_err(),
            LedgerError::Unknown(_)
        ));
        l.post(resv(1), &[amt(100_000)]).unwrap();
        // void on an already-posted reservation is a state error.
        assert!(matches!(
            l.void(resv(1)).unwrap_err(),
            LedgerError::WrongState { .. }
        ));
        // void_reorg only applies to a posted reservation.
        assert!(matches!(
            l.void_reorg(resv(9)).unwrap_err(),
            LedgerError::Unknown(_)
        ));
    }

    #[test]
    fn post_validates_fill_shape_without_mutating() {
        let mut l = funded();
        l.reserve(reservation(1, vec![source(1, 1, 3, 100_000)]))
            .unwrap();
        assert!(matches!(
            l.post(resv(1), &[]).unwrap_err(),
            LedgerError::FillCountMismatch { got: 0, want: 1 }
        ));
        assert!(matches!(
            l.post(resv(1), &[amt(100_001)]).unwrap_err(),
            LedgerError::FillExceedsReserved { .. }
        ));
        assert_eq!(
            l.reservation(&resv(1)).unwrap().state,
            ReservationState::Pending
        );
        assert_eq!(l.pending(&wallet(1, 3)), amt(100_000));
    }

    // ---- Property: conservation + no over-promise under random command streams. ----

    #[derive(Debug, Clone)]
    enum Command {
        Reserve {
            id: u8,
            sources: Vec<(u8, u8, u8, u64)>,
        },
        Post {
            id: u8,
            fracs: Vec<u8>,
        },
        Void {
            id: u8,
        },
        Expire {
            id: u8,
        },
        Reorg {
            id: u8,
        },
    }

    // Universe small enough to enumerate every touched account: makers {1,2}, strategies {1,2},
    // tokens {3,4}, reservation ids 0..8.
    fn seeded_accounts() -> Vec<AccountKey> {
        let mut accounts = Vec::new();
        for m in 1..=2 {
            for t in 3..=4 {
                accounts.push(wallet(m, t));
                for s in 1..=2 {
                    accounts.push(virt(m, s, t));
                }
            }
        }
        accounts
    }
    fn seeded_ledger() -> Ledger {
        let mut l = Ledger::default();
        for account in seeded_accounts() {
            let budget = match account {
                AccountKey::WalletBudget { .. } => amt(1_000_000),
                AccountKey::StrategyVirtual { .. } => amt(600_000),
            };
            l.set_budget(account, budget);
        }
        l
    }

    fn command() -> impl Strategy<Value = Command> {
        let a_source = (1u8..=2, 1u8..=2, 3u8..=4, 0u64..=400_000);
        prop_oneof![
            3 => (0u8..8, prop::collection::vec(a_source, 1..=2))
                .prop_map(|(id, sources)| Command::Reserve { id, sources }),
            2 => (0u8..8, prop::collection::vec(0u8..=255, 0..=3))
                .prop_map(|(id, fracs)| Command::Post { id, fracs }),
            1 => (0u8..8).prop_map(|id| Command::Void { id }),
            1 => (0u8..8).prop_map(|id| Command::Expire { id }),
            1 => (0u8..8).prop_map(|id| Command::Reorg { id }),
        ]
    }

    // A parallel record of what each reservation should contribute, updated only when the engine
    // accepts the command — so the oracle mirrors the engine's own admission decisions exactly.
    type Mirror = BTreeMap<ReservationId, (Vec<ReservationSource>, ReservationState, Vec<U256>)>;

    fn drive(l: &mut Ledger, mirror: &mut Mirror, cmd: Command) {
        match cmd {
            Command::Reserve { id, sources } => {
                let sources: Vec<ReservationSource> = sources
                    .into_iter()
                    .map(|(m, s, t, a)| source(m, s, t, a))
                    .collect();
                if l.reserve(reservation(id, sources.clone())).is_ok() {
                    mirror.insert(resv(id), (sources, ReservationState::Pending, Vec::new()));
                }
            }
            Command::Post { id, fracs } => {
                // fills ≤ reserved (frac/255) when the shapes line up, else raw values the engine rejects.
                let fills: Vec<U256> = match mirror.get(&resv(id)) {
                    Some((sources, ReservationState::Pending, _))
                        if fracs.len() == sources.len() =>
                    {
                        sources
                            .iter()
                            .zip(&fracs)
                            .map(|(source, &f)| source.amount * amt(f as u64) / amt(255))
                            .collect()
                    }
                    _ => fracs.iter().map(|&f| amt(f as u64)).collect(),
                };
                if l.post(resv(id), &fills).is_ok() {
                    if let Some(entry) = mirror.get_mut(&resv(id)) {
                        entry.1 = ReservationState::Posted;
                        entry.2 = fills;
                    }
                }
            }
            Command::Void { id } => {
                if l.void(resv(id)).is_ok() {
                    if let Some(entry) = mirror.get_mut(&resv(id)) {
                        entry.1 = ReservationState::Voided;
                    }
                }
            }
            Command::Expire { id } => {
                if l.expire(resv(id)).is_ok() {
                    if let Some(entry) = mirror.get_mut(&resv(id)) {
                        entry.1 = ReservationState::Expired;
                    }
                }
            }
            Command::Reorg { id } => {
                if l.void_reorg(resv(id)).is_ok() {
                    if let Some(entry) = mirror.get_mut(&resv(id)) {
                        entry.1 = ReservationState::ReorgOpen;
                    }
                }
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::default() })]
        #[test]
        fn conserves_and_never_over_promises(cmds in prop::collection::vec(command(), 0..400)) {
            let mut l = seeded_ledger();
            let mut mirror = Mirror::new();
            for cmd in cmds {
                drive(&mut l, &mut mirror, cmd);

                // Recompute expected holds from scratch: only PENDING reservations hold `pending`,
                // only POSTED reservations hold `consumed` (their recorded fills).
                let mut want_pending: BTreeMap<AccountKey, U256> = BTreeMap::new();
                let mut want_consumed: BTreeMap<AccountKey, U256> = BTreeMap::new();
                for (sources, state, fills) in mirror.values() {
                    match state {
                        ReservationState::Pending => {
                            for source in sources {
                                for account in source.accounts() {
                                    let e = want_pending.entry(account).or_insert(U256::ZERO);
                                    *e = e.saturating_add(source.amount);
                                }
                            }
                        }
                        ReservationState::Posted => {
                            for (i, source) in sources.iter().enumerate() {
                                let fill = fills.get(i).copied().unwrap_or(U256::ZERO);
                                for account in source.accounts() {
                                    let e = want_consumed.entry(account).or_insert(U256::ZERO);
                                    *e = e.saturating_add(fill);
                                }
                            }
                        }
                        _ => {}
                    }
                }

                for account in seeded_accounts() {
                    let pending = l.pending(&account);
                    let consumed = l.consumed(&account);
                    let budget = l.budget(&account);
                    // Incremental accounting matches the from-scratch fold.
                    prop_assert_eq!(pending, want_pending.get(&account).copied().unwrap_or(U256::ZERO));
                    prop_assert_eq!(consumed, want_consumed.get(&account).copied().unwrap_or(U256::ZERO));
                    // Never promise past the budget, and `available` is exact (no clamp lost value).
                    prop_assert!(pending.saturating_add(consumed) <= budget);
                    prop_assert_eq!(l.available(&account), budget - pending - consumed);
                }
            }
        }
    }
}
