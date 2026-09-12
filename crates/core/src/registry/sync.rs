//! Drives one chain's registry: fetch, drop already-seen events, insert, fold the new ones into the
//! snapshot, then advance the cursor. The store's unique key makes application exactly-once; the moka
//! cache pre-filters overlap re-scans.

use std::sync::Arc;

use moka::sync::Cache;

use super::SharedSnapshot;
use crate::deps::registry::{ChainSource, EventStore};
use crate::obs::warn;
use crate::primitives::registry::{EventCursor, Snapshot};
use crate::primitives::{ChainConfig, ChainId};
use crate::SolventError;

pub struct RegistrySync {
    chain: ChainId,
    start_block: u64,
    overlap_blocks: u64,
    source: Arc<dyn ChainSource>,
    store: Arc<dyn EventStore>,
    snapshot: Arc<SharedSnapshot>,
    seen: Cache<EventCursor, ()>,
}

impl RegistrySync {
    pub fn new(
        config: &ChainConfig,
        source: Arc<dyn ChainSource>,
        store: Arc<dyn EventStore>,
        snapshot: Arc<SharedSnapshot>,
    ) -> Self {
        let seen = Cache::builder().time_to_live(config.dedup_ttl()).build();
        Self {
            chain: config.chain(),
            start_block: config.start_block(),
            overlap_blocks: config.overlap_blocks(),
            source,
            store,
            snapshot,
            seen,
        }
    }

    /// Rebuild the snapshot from the durable event log — cold start and restart.
    pub async fn recover(&self) -> Result<(), SolventError> {
        let mut snapshot = Snapshot::default();
        for event in self.store.events(self.chain).await? {
            snapshot.apply(event.event);
        }
        self.snapshot.store(snapshot);
        Ok(())
    }

    /// One sync cycle, scanning up to `to_block`.
    pub async fn sync_once(&self, to_block: u64) -> Result<(), SolventError> {
        let resume = self.store.cursor(self.chain).await?;
        // Scan from the cursor rewound by the overlap window (or the start block on the first cycle).
        let from_block = resume.map_or(self.start_block, |c| {
            c.block_number.saturating_sub(self.overlap_blocks)
        });
        // A head behind the cursor means the chain is not the one this cursor was recorded
        // against — a reset node, or a devnet redeployed under the same id. Fetching would ask
        // for an inverted range and fail every tick while the snapshot silently went stale, so
        // say so once per cycle and wait for the head to catch up.
        if from_block > to_block {
            warn!(
                from_block,
                to_block, "chain head is behind the stored cursor; skipping this sync cycle"
            );
            return Ok(());
        }
        let fetched = self.source.fetch(from_block, to_block).await?;

        // The `seen` cache keys on `(block, log)`, not the block hash, so it can't spot a reorg replacement.
        let fresh: Vec<_> = fetched
            .into_iter()
            .filter(|event| event.cursor().is_some_and(|c| self.seen.get(&c).is_none()))
            .collect();

        let mut inserted = self.store.insert(self.chain, &fresh).await?;

        // Everything scanned is now known; only the newly-inserted rows change the
        // snapshot (the store deduplicated the rest).
        for event in &fresh {
            if let Some(c) = event.cursor() {
                self.seen.insert(c, ());
            }
        }
        if !inserted.is_empty() {
            // Fold in cursor order so a `Shipped` precedes its `Pushed`, whatever order the source gave.
            inserted.sort_by_key(|event| (event.block_number, event.log_index));
            let mut snapshot = (*self.snapshot.load()).clone();
            for event in inserted {
                snapshot.apply(event.event);
            }
            self.snapshot.store(snapshot);
        }

        // Commit progress last, never below the recorded cursor (a stale tip can't rewind it).
        let progress = resume.map_or(to_block, |c| to_block.max(c.block_number));
        self.store
            .save_cursor(
                self.chain,
                EventCursor {
                    block_number: progress,
                    log_index: 0,
                },
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deps::registry::{ChainSourceError, RecordedEvent, StoreError};
    use crate::primitives::registry::{AquaEvent, EventExt, StrategyKey};
    use crate::primitives::{MakerId, StrategyHash};
    use alloy_primitives::{Address, B256, U256};
    use async_trait::async_trait;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    fn maker(n: u8) -> MakerId {
        MakerId(Address::from([n; 20]))
    }
    fn app() -> Address {
        Address::from([0xAA; 20])
    }
    fn hash(n: u8) -> StrategyHash {
        StrategyHash(B256::from([n; 32]))
    }
    fn token(n: u8) -> Address {
        Address::from([n; 20])
    }
    fn key(s: u8) -> StrategyKey {
        StrategyKey {
            maker: maker(s),
            app: app(),
            strategy_hash: hash(s),
        }
    }
    fn shipped(s: u8) -> AquaEvent {
        AquaEvent::Shipped {
            maker: maker(s),
            app: app(),
            strategy_hash: hash(s),
            strategy: alloy_primitives::Bytes::from(vec![s]),
        }
    }
    fn pushed(s: u8, t: u8, amt: u64) -> AquaEvent {
        AquaEvent::Pushed {
            maker: maker(s),
            app: app(),
            strategy_hash: hash(s),
            token: token(t),
            amount: U256::from(amt),
        }
    }
    fn ext(block: u64, log: u64, event: AquaEvent) -> EventExt<AquaEvent> {
        EventExt {
            event,
            address: Address::ZERO,
            block_hash: Some(B256::from([block as u8; 32])),
            block_number: Some(block),
            transaction_hash: None,
            transaction_index: None,
            log_index: Some(log),
            removed: false,
        }
    }

    struct FakeSource(Mutex<Vec<EventExt<AquaEvent>>>);
    #[async_trait]
    impl ChainSource for FakeSource {
        async fn fetch(
            &self,
            from: u64,
            to: u64,
        ) -> Result<Vec<EventExt<AquaEvent>>, ChainSourceError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|e| {
                    let b = e.block_number.unwrap();
                    b >= from && b <= to
                })
                .cloned()
                .collect())
        }
    }

    #[derive(Default)]
    struct FakeStore {
        events: Mutex<BTreeMap<(u64, u64), EventExt<AquaEvent>>>,
        cursor: Mutex<Option<EventCursor>>,
    }
    #[async_trait]
    impl EventStore for FakeStore {
        async fn cursor(&self, _chain: ChainId) -> Result<Option<EventCursor>, StoreError> {
            Ok(*self.cursor.lock().unwrap())
        }
        async fn insert(
            &self,
            _chain: ChainId,
            events: &[EventExt<AquaEvent>],
        ) -> Result<Vec<EventExt<AquaEvent>>, StoreError> {
            let mut map = self.events.lock().unwrap();
            let mut inserted = Vec::new();
            for e in events {
                let c = e.cursor().ok_or(StoreError::Unpositioned)?;
                if map
                    .insert((c.block_number, c.log_index), e.clone())
                    .is_none()
                {
                    inserted.push(e.clone());
                }
            }
            Ok(inserted)
        }
        async fn save_cursor(
            &self,
            _chain: ChainId,
            cursor: EventCursor,
        ) -> Result<(), StoreError> {
            *self.cursor.lock().unwrap() = Some(cursor);
            Ok(())
        }
        async fn events(&self, _chain: ChainId) -> Result<Vec<EventExt<AquaEvent>>, StoreError> {
            Ok(self.events.lock().unwrap().values().cloned().collect())
        }
        async fn history(
            &self,
            _chain: ChainId,
            hash: StrategyHash,
        ) -> Result<Vec<EventExt<AquaEvent>>, StoreError> {
            Ok(self
                .events
                .lock()
                .unwrap()
                .values()
                .filter(|e| e.event.key().strategy_hash == hash)
                .cloned()
                .collect())
        }
        async fn recent(
            &self,
            _chain: ChainId,
            _before: Option<EventCursor>,
            _limit: u32,
        ) -> Result<Vec<RecordedEvent>, StoreError> {
            Ok(Vec::new())
        }
        async fn count_since(&self, _chain: ChainId, _since: u64) -> Result<u64, StoreError> {
            Ok(0)
        }
    }

    fn sync_with(source: Arc<FakeSource>) -> (RegistrySync, Arc<SharedSnapshot>, Arc<FakeStore>) {
        let snapshot = Arc::new(SharedSnapshot::default());
        let store = Arc::new(FakeStore::default());
        let sync = RegistrySync::new(
            &ChainConfig::ethereum(0),
            source,
            store.clone(),
            snapshot.clone(),
        );
        (sync, snapshot, store)
    }

    #[tokio::test]
    async fn applies_new_events_once_across_overlap_rescans() {
        let source = Arc::new(FakeSource(Mutex::new(vec![
            ext(10, 0, shipped(1)),
            ext(10, 1, pushed(1, 1, 1000)),
            ext(10, 2, pushed(1, 2, 1000)),
        ])));
        let (sync, snapshot, _store) = sync_with(source);

        sync.sync_once(10).await.unwrap();
        let s = snapshot.load();
        let strat = s.strategy(&key(1)).unwrap();
        assert_eq!(strat.balance(&token(1)), U256::from(1000u64));
        assert_eq!(strat.balance(&token(2)), U256::from(1000u64));

        // Re-scanning the same range must not double-count.
        sync.sync_once(10).await.unwrap();
        let s = snapshot.load();
        assert_eq!(
            s.strategy(&key(1)).unwrap().balance(&token(1)),
            U256::from(1000u64)
        );
    }

    #[tokio::test]
    async fn applies_late_arriving_event_below_the_tip() {
        // First scan of block 10 omits log 1 (RPC eventual consistency).
        let source = Arc::new(FakeSource(Mutex::new(vec![
            ext(10, 0, shipped(1)),
            ext(10, 2, pushed(1, 2, 1000)),
        ])));
        let (sync, snapshot, _store) = sync_with(source.clone());

        sync.sync_once(10).await.unwrap();
        let s = snapshot.load();
        assert_eq!(
            s.strategy(&key(1)).unwrap().balance(&token(2)),
            U256::from(1000u64)
        );
        assert_eq!(s.strategy(&key(1)).unwrap().balance(&token(1)), U256::ZERO);

        // Next cycle the RPC also returns the previously-missing log 1.
        *source.0.lock().unwrap() = vec![
            ext(10, 0, shipped(1)),
            ext(10, 1, pushed(1, 1, 500)),
            ext(10, 2, pushed(1, 2, 1000)),
        ];
        sync.sync_once(10).await.unwrap();
        let s = snapshot.load();
        // The late event lands; the already-seen ones do not re-apply.
        assert_eq!(
            s.strategy(&key(1)).unwrap().balance(&token(1)),
            U256::from(500u64)
        );
        assert_eq!(
            s.strategy(&key(1)).unwrap().balance(&token(2)),
            U256::from(1000u64)
        );
    }

    #[tokio::test]
    async fn folds_inserted_in_cursor_order_regardless_of_source_order() {
        // The source hands the Pushed back before its Shipped; the fold must still register the
        // strategy first, or the balance is lost.
        let source = Arc::new(FakeSource(Mutex::new(vec![
            ext(10, 1, pushed(1, 1, 500)),
            ext(10, 0, shipped(1)),
        ])));
        let (sync, snapshot, _store) = sync_with(source);
        sync.sync_once(10).await.unwrap();
        assert_eq!(
            snapshot
                .load()
                .strategy(&key(1))
                .unwrap()
                .balance(&token(1)),
            U256::from(500u64)
        );
    }

    #[tokio::test]
    async fn does_not_regress_the_cursor_on_a_stale_to_block() {
        let source = Arc::new(FakeSource(Mutex::new(vec![ext(20, 0, shipped(1))])));
        let (sync, _snapshot, store) = sync_with(source);
        sync.sync_once(20).await.unwrap();
        sync.sync_once(10).await.unwrap(); // a stale tip must not rewind progress
        assert_eq!(
            store.cursor.lock().unwrap().clone(),
            Some(EventCursor {
                block_number: 20,
                log_index: 0,
            })
        );
    }
}
