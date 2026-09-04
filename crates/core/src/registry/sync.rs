//! Drives one chain's registry: fetch a block range, drop events already seen,
//! insert the rest, fold the newly-inserted ones into the shared snapshot, and
//! only then advance the cursor. Correctness rests on the store's unique key (only
//! genuinely-new rows come back to apply, so overlap re-scans and late RPC
//! re-deliveries are exactly once); the moka cache is a pre-filter that spares the
//! store the redundant overlap writes each cycle. Reorg handling is a later
//! addition — the provenance it needs is already carried and stored.

use std::sync::Arc;

use moka::sync::Cache;

use super::SharedSnapshot;
use crate::deps::registry::{ChainSource, Store};
use crate::primitives::registry::{EventCursor, Snapshot};
use crate::primitives::{ChainConfig, ChainId};
use crate::SolventError;

pub struct RegistrySync {
    chain: ChainId,
    start_block: u64,
    overlap_blocks: u64,
    source: Arc<dyn ChainSource>,
    store: Arc<dyn Store>,
    snapshot: Arc<SharedSnapshot>,
    seen: Cache<EventCursor, ()>,
}

impl RegistrySync {
    pub fn new(
        config: &ChainConfig,
        source: Arc<dyn ChainSource>,
        store: Arc<dyn Store>,
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
        let from_block = self.resume_from().await?;
        let fetched = self.source.fetch(from_block, to_block).await?;

        let fresh: Vec<_> = fetched
            .into_iter()
            .filter(|event| event.cursor().is_some_and(|c| self.seen.get(&c).is_none()))
            .collect();

        let inserted = self.store.insert(self.chain, &fresh).await?;

        // Everything scanned is now known; only the newly-inserted rows change the
        // snapshot (the store deduplicated the rest).
        for event in &fresh {
            if let Some(c) = event.cursor() {
                self.seen.insert(c, ());
            }
        }
        if !inserted.is_empty() {
            let mut snapshot = (*self.snapshot.load()).clone();
            for event in inserted {
                snapshot.apply(event.event);
            }
            self.snapshot.store(snapshot);
        }

        // Commit progress last: the events are durable and the snapshot reflects
        // them, so advancing the cursor can't skip unprocessed work.
        let cursor = EventCursor {
            block_number: to_block,
            log_index: 0,
        };
        self.store.save_cursor(self.chain, cursor).await?;
        Ok(())
    }

    /// Where the next scan starts: the cursor rewound by the overlap window, or
    /// the configured start block before the first cycle.
    async fn resume_from(&self) -> Result<u64, SolventError> {
        let cursor = self.store.cursor(self.chain).await?;
        Ok(cursor.map_or(self.start_block, |c| {
            c.block_number.saturating_sub(self.overlap_blocks)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deps::registry::{ChainSourceError, StoreError};
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
    impl Store for FakeStore {
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
    }

    fn sync_with(source: Arc<FakeSource>) -> (RegistrySync, Arc<SharedSnapshot>) {
        let snapshot = Arc::new(SharedSnapshot::default());
        let sync = RegistrySync::new(
            &ChainConfig::ethereum(0),
            source,
            Arc::new(FakeStore::default()),
            snapshot.clone(),
        );
        (sync, snapshot)
    }

    #[tokio::test]
    async fn applies_new_events_once_across_overlap_rescans() {
        let source = Arc::new(FakeSource(Mutex::new(vec![
            ext(10, 0, shipped(1)),
            ext(10, 1, pushed(1, 1, 1000)),
            ext(10, 2, pushed(1, 2, 1000)),
        ])));
        let (sync, snapshot) = sync_with(source);

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
        let (sync, snapshot) = sync_with(source.clone());

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
}
