//! The ingest pipeline: fan many order feeds into one clean, deduped `Intent` stream. Each raw order
//! is normalized by its protocol's normalizer, admitted by cheap edge checks (chain, deadline,
//! amounts), and deduped on the order hash before it reaches the bounded output channel — the
//! backpressure seam to the decision loop. On-chain concerns (cosignature, exact fillability) are
//! enforced later by the reactor, not here.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{select_all, StreamExt};
use moka::sync::Cache;
use tokio::sync::mpsc;

use crate::deps::ingest::{Normalizer, OrderFeed};
use crate::deps::ledger::Clock;
use crate::obs::debug;
use crate::primitives::ingest::{Intent, ProtocolId, RawOrder};
use crate::primitives::{ChainId, IntentId};

pub struct IngestPipeline {
    normalizers: BTreeMap<ProtocolId, Arc<dyn Normalizer>>,
    dedup: Cache<IntentId, ()>,
    clock: Arc<dyn Clock>,
    supported_chains: BTreeSet<ChainId>,
}

impl IngestPipeline {
    pub fn new(
        normalizers: BTreeMap<ProtocolId, Arc<dyn Normalizer>>,
        dedup_ttl: Duration,
        clock: Arc<dyn Clock>,
        supported_chains: BTreeSet<ChainId>,
    ) -> IngestPipeline {
        IngestPipeline {
            normalizers,
            dedup: Cache::builder().time_to_live(dedup_ttl).build(),
            clock,
            supported_chains,
        }
    }

    /// Fan in every feed, process each order, and send survivors to `out`. Runs until every feed's
    /// stream ends, or the receiver is dropped. The caller owns `out`; its capacity is the backpressure.
    pub async fn run(&self, feeds: Vec<Arc<dyn OrderFeed>>, out: mpsc::Sender<Intent>) {
        if feeds.is_empty() {
            return;
        }
        let mut merged = select_all(feeds.iter().map(|f| f.stream()));
        while let Some(raw) = merged.next().await {
            let Some(intent) = self.process(raw) else {
                continue;
            };
            if out.send(intent).await.is_err() {
                break; // the decision loop is gone
            }
        }
    }

    /// Normalize, admit, and dedup one raw order. `None` when it is dropped (reason logged).
    fn process(&self, raw: RawOrder) -> Option<Intent> {
        let Some(normalizer) = self.normalizers.get(&raw.protocol) else {
            debug!("ingest drop: no normalizer for {:?}", raw.protocol);
            return None;
        };
        let intent = match normalizer.normalize(&raw) {
            Ok(intent) => intent,
            Err(_) => {
                debug!("ingest drop: malformed {:?} order", raw.protocol);
                return None;
            }
        };
        if !self.admit(&intent) {
            return None;
        }
        if self.dedup.get(&intent.id).is_some() {
            debug!("ingest drop {}: duplicate", intent.id);
            return None;
        }
        self.dedup.insert(intent.id, ());
        Some(intent)
    }

    /// The protocol-agnostic edge checks: on a supported chain, still live, asking for real amounts.
    /// On-chain concerns (cosignature, exact fillability) are enforced later by the reactor.
    fn admit(&self, intent: &Intent) -> bool {
        let now = self.clock.now_unix();
        if !self.supported_chains.contains(&intent.origin_chain) {
            debug!("ingest drop {}: unsupported chain", intent.id);
            return false;
        }
        if intent.deadline <= now {
            debug!("ingest drop {}: expired", intent.id);
            return false;
        }
        if intent.outputs.is_empty() {
            debug!("ingest drop {}: no outputs", intent.id);
            return false;
        }
        if intent.input.curve.amount_at(now).is_zero() {
            debug!("ingest drop {}: zero input", intent.id);
            return false;
        }
        if intent
            .outputs
            .iter()
            .any(|o| o.curve.amount_at(now).is_zero())
        {
            debug!("ingest drop {}: zero output", intent.id);
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use alloy_primitives::{Address, Bytes, B256, U256};
    use futures::stream::BoxStream;

    use super::*;
    use crate::deps::ingest::NormalizeError;
    use crate::primitives::ingest::{AmountCurve, IntentInput, IntentOutput};

    fn addr(n: u8) -> Address {
        Address::from([n; 20])
    }

    fn intent(id: u8, chain: ChainId, deadline: u64, in_amt: u64, out_amt: u64) -> Intent {
        Intent::new(
            IntentId(B256::from([id; 32])),
            ProtocolId::UniswapXV2,
            IntentInput::new(addr(1), AmountCurve::scalar(U256::from(in_amt))),
            vec![IntentOutput::new(
                addr(2),
                AmountCurve::scalar(U256::from(out_amt)),
                addr(3),
            )],
            deadline,
            None,
            addr(4),
            chain,
            Bytes::new(),
            Bytes::new(),
            0,
        )
    }

    fn raw(tag: u8) -> RawOrder {
        RawOrder::new(
            ProtocolId::UniswapXV2,
            ChainId(1),
            Bytes::from(vec![tag]),
            Bytes::new(),
            0,
        )
    }

    /// Maps each raw payload to a pre-built intent; a payload with no mapping fails to normalize.
    struct MapNorm(HashMap<Bytes, Intent>);
    impl Normalizer for MapNorm {
        fn normalize(&self, raw: &RawOrder) -> Result<Intent, NormalizeError> {
            self.0
                .get(&raw.payload)
                .cloned()
                .ok_or(NormalizeError::Decode(raw.protocol))
        }
    }

    struct FakeFeed(Vec<RawOrder>);
    impl OrderFeed for FakeFeed {
        fn stream(&self) -> BoxStream<'static, RawOrder> {
            futures::stream::iter(self.0.clone()).boxed()
        }
    }

    struct FixedClock(u64);
    impl Clock for FixedClock {
        fn now_unix(&self) -> u64 {
            self.0
        }
    }

    fn feed(orders: Vec<RawOrder>) -> Arc<dyn OrderFeed> {
        Arc::new(FakeFeed(orders))
    }

    fn pipeline(map: HashMap<Bytes, Intent>) -> IngestPipeline {
        let mut norms: BTreeMap<ProtocolId, Arc<dyn Normalizer>> = BTreeMap::new();
        norms.insert(ProtocolId::UniswapXV2, Arc::new(MapNorm(map)));
        IngestPipeline::new(
            norms,
            Duration::from_secs(60),
            Arc::new(FixedClock(1000)),
            BTreeSet::from([ChainId(1)]),
        )
    }

    async fn drain(p: &IngestPipeline, feeds: Vec<Arc<dyn OrderFeed>>) -> Vec<Intent> {
        let (tx, mut rx) = mpsc::channel(64);
        p.run(feeds, tx).await;
        let mut got = Vec::new();
        while let Some(i) = rx.recv().await {
            got.push(i);
        }
        got
    }

    #[tokio::test]
    async fn dedup_drops_a_repeat_hash() {
        // Two distinct payloads that normalize to the same order hash → one survives.
        let map = HashMap::from([
            (Bytes::from(vec![1]), intent(7, ChainId(1), 2000, 1, 1)),
            (Bytes::from(vec![2]), intent(7, ChainId(1), 2000, 1, 1)),
        ]);
        let got = drain(&pipeline(map), vec![feed(vec![raw(1), raw(2)])]).await;
        assert_eq!(got.len(), 1);
    }

    #[tokio::test]
    async fn admits_valid_and_drops_invalid() {
        // now = 1000, supported chain = 1.
        let map = HashMap::from([
            (Bytes::from(vec![1]), intent(1, ChainId(1), 2000, 5, 5)), // valid
            (Bytes::from(vec![2]), intent(2, ChainId(1), 500, 5, 5)),  // expired
            (Bytes::from(vec![3]), intent(3, ChainId(9), 2000, 5, 5)), // unsupported chain
            (Bytes::from(vec![4]), intent(4, ChainId(1), 2000, 5, 0)), // zero output
        ]);
        let feeds = vec![feed(vec![raw(1), raw(2), raw(3), raw(4)])];
        let got = drain(&pipeline(map), feeds).await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, IntentId(B256::from([1u8; 32])));
    }

    #[tokio::test]
    async fn drops_unknown_protocol_and_normalize_errors() {
        // Empty normalizer set → no dispatch target; and a payload with no mapping → normalize error.
        let no_norms = IngestPipeline::new(
            BTreeMap::new(),
            Duration::from_secs(60),
            Arc::new(FixedClock(1000)),
            BTreeSet::from([ChainId(1)]),
        );
        assert!(drain(&no_norms, vec![feed(vec![raw(1)])]).await.is_empty());

        let unmapped = pipeline(HashMap::new()); // normalizer present, but no payload mapping
        assert!(drain(&unmapped, vec![feed(vec![raw(1)])]).await.is_empty());
    }

    #[tokio::test]
    async fn fans_in_multiple_feeds() {
        let map = HashMap::from([
            (Bytes::from(vec![1]), intent(1, ChainId(1), 2000, 1, 1)),
            (Bytes::from(vec![2]), intent(2, ChainId(1), 2000, 1, 1)),
        ]);
        let feeds = vec![feed(vec![raw(1)]), feed(vec![raw(2)])];
        let got = drain(&pipeline(map), feeds).await;
        assert_eq!(got.len(), 2);
    }
}
