//! The ingest pipeline: fan many order feeds into one clean, deduped `Intent` stream. Each raw order
//! is normalized by its protocol's normalizer, admitted by cheap edge checks (chain, deadline,
//! amounts), and deduped on the order hash before it reaches the bounded output channel — the
//! backpressure seam to the decision loop. On-chain concerns (cosignature, exact fillability) are
//! enforced later by the reactor, not here.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::Address;
use futures::stream::{select_all, StreamExt};
use moka::sync::Cache;
use tokio::sync::mpsc;

use crate::deps::ingest::{Normalizer, OrderFeed};
use crate::deps::ledger::Clock;
use crate::obs::debug;
use crate::primitives::ingest::{Intent, ProtocolId, RawOrder};
use crate::primitives::{ChainId, IntentId};

/// What this deployment is willing to take on. Orders arrive from a public feed and are shaped by
/// whoever posted them, so admission is an allow-list of what we can settle rather than a deny-list
/// of what has bitten us.
///
/// Deliberately exhaustive: a new rule must break every construction site rather than default to
/// admitting whatever it governs.
#[derive(Clone, Debug)]
pub struct Admission {
    pub supported_chains: BTreeSet<ChainId>,
    /// Tokens this resolver will touch on either side. Sourcing runs against maker positions, so a
    /// token no maker has shipped is unfillable anyway — and the list doubles as the exclusion for
    /// fee-on-transfer, rebasing, and blocklisting tokens, each of which settles for less than the
    /// reactor demands and reverts inside the filler's own balance guard.
    pub tokens: BTreeSet<Address>,
    /// Ceiling on an order's output legs. Each one is a separate token to source and a term in the
    /// filler's quadratic output scan, so an order with hundreds is an out-of-gas bomb.
    pub max_outputs: usize,
}

pub struct IngestPipeline {
    normalizers: BTreeMap<ProtocolId, Arc<dyn Normalizer>>,
    dedup: Cache<IntentId, ()>,
    clock: Arc<dyn Clock>,
    admission: Admission,
}

impl IngestPipeline {
    pub fn new(
        normalizers: BTreeMap<ProtocolId, Arc<dyn Normalizer>>,
        dedup_ttl: Duration,
        dedup_capacity: u64,
        clock: Arc<dyn Clock>,
        admission: Admission,
    ) -> IngestPipeline {
        IngestPipeline {
            normalizers,
            // Bounded as well as expiring: an order stream we do not control could otherwise pin
            // arbitrarily many hashes for a whole TTL.
            dedup: Cache::builder()
                .time_to_live(dedup_ttl)
                .max_capacity(dedup_capacity)
                .build(),
            clock,
            admission,
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
        if !self.is_admissible(&intent) {
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
    fn is_admissible(&self, intent: &Intent) -> bool {
        let now = self.clock.now_unix();
        if !self
            .admission
            .supported_chains
            .contains(&intent.origin_chain)
        {
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
        if intent.outputs.len() > self.admission.max_outputs {
            debug!("ingest drop {}: too many outputs", intent.id);
            return false;
        }
        // The zero address is the settlement layer's native-currency sentinel, and the filler
        // settles ERC20 only: the reactor would pay a native output out of its own balance, which
        // ours never funds.
        if intent.outputs.iter().any(|o| o.token == Address::ZERO)
            || intent.input.token == Address::ZERO
        {
            debug!("ingest drop {}: native currency leg", intent.id);
            return false;
        }
        // Legs in different tokens would each need their own route and reservation, succeeding or
        // unwinding together. No live order is shaped that way, so this declines rather than
        // sourcing one token and meeting the rest on chain.
        if intent.delivery(now).is_none() {
            debug!("ingest drop {}: outputs span several tokens", intent.id);
            return false;
        }
        let tokens_allowed = self.admission.tokens.contains(&intent.input.token)
            && intent
                .outputs
                .iter()
                .all(|o| self.admission.tokens.contains(&o.token));
        if !tokens_allowed {
            debug!("ingest drop {}: token not admitted", intent.id);
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
    use crate::primitives::ingest::{AmountCurve, IntentInput, IntentOutput, IntentParts};

    fn addr(n: u8) -> Address {
        Address::from([n; 20])
    }

    fn intent(id: u8, chain: ChainId, deadline: u64, in_amt: u64, out_amt: u64) -> Intent {
        intent_with(id, chain, deadline, in_amt, out_amt, vec![addr(2)])
    }

    fn intent_with(
        id: u8,
        chain: ChainId,
        deadline: u64,
        in_amt: u64,
        out_amt: u64,
        output_tokens: Vec<Address>,
    ) -> Intent {
        let outputs = output_tokens
            .into_iter()
            .map(|token| {
                IntentOutput::new(token, AmountCurve::scalar(U256::from(out_amt)), addr(3))
            })
            .collect();
        Intent::new(IntentParts {
            deadline,
            settler: addr(4),
            ..IntentParts::new(
                IntentId(B256::from([id; 32])),
                ProtocolId::UniswapXV2,
                addr(9),
                IntentInput::new(addr(1), AmountCurve::scalar(U256::from(in_amt))),
                outputs,
                chain,
            )
        })
    }

    /// Admits chain 1 and the tokens the fixtures use.
    fn admission() -> Admission {
        Admission {
            supported_chains: BTreeSet::from([ChainId(1)]),
            tokens: BTreeSet::from([addr(1), addr(2)]),
            max_outputs: 4,
        }
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
            1024,
            Arc::new(FixedClock(1000)),
            admission(),
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

    /// One case per rule an attacker-shaped order can trip. Each is an order we would otherwise
    /// spend gas discovering we cannot settle.
    #[tokio::test]
    async fn drops_orders_this_filler_cannot_settle() {
        let native = intent_with(1, ChainId(1), 2000, 5, 5, vec![Address::ZERO]);
        let unlisted = intent_with(2, ChainId(1), 2000, 5, 5, vec![addr(8)]);
        let too_many = intent_with(3, ChainId(1), 2000, 5, 5, vec![addr(2); 5]);
        let ok = intent_with(4, ChainId(1), 2000, 5, 5, vec![addr(2); 4]);
        let map = HashMap::from([
            (Bytes::from(vec![1]), native),
            (Bytes::from(vec![2]), unlisted),
            (Bytes::from(vec![3]), too_many),
            (Bytes::from(vec![4]), ok),
        ]);
        let feeds = vec![feed(vec![raw(1), raw(2), raw(3), raw(4)])];
        let got = drain(&pipeline(map), feeds).await;
        assert_eq!(got.len(), 1, "only the four-output listed order survives");
        assert_eq!(got[0].id, IntentId(B256::from([4u8; 32])));
    }

    /// Legs in one token are fine however many there are — that is the swapper-plus-fee shape. Legs
    /// in different tokens are declined, since each would need its own route and reservation.
    #[tokio::test]
    async fn admits_repeated_tokens_but_not_mixed_ones() {
        let same = intent_with(1, ChainId(1), 2000, 5, 5, vec![addr(2), addr(2)]);
        let mixed = intent_with(2, ChainId(1), 2000, 5, 5, vec![addr(2), addr(1)]);
        let map = HashMap::from([(Bytes::from(vec![1]), same), (Bytes::from(vec![2]), mixed)]);
        let got = drain(&pipeline(map), vec![feed(vec![raw(1), raw(2)])]).await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, IntentId(B256::from([1u8; 32])));
    }

    /// The native-currency rule, isolated. Both other native tests are also satisfied by the token
    /// allow-list, so deleting this rule breaks neither: here the zero address is *admitted* by the
    /// allow-list, leaving the native check as the only thing that can decline the order.
    #[tokio::test]
    async fn drops_a_native_leg_even_when_the_zero_address_is_admitted() {
        let mut norms: BTreeMap<ProtocolId, Arc<dyn Normalizer>> = BTreeMap::new();
        let native_out = intent_with(1, ChainId(1), 2000, 5, 5, vec![Address::ZERO]);
        let mut native_in = intent(2, ChainId(1), 2000, 5, 5);
        native_in.input = IntentInput::new(Address::ZERO, AmountCurve::scalar(U256::from(5u64)));
        let ok = intent_with(3, ChainId(1), 2000, 5, 5, vec![addr(2)]);
        norms.insert(
            ProtocolId::UniswapXV2,
            Arc::new(MapNorm(HashMap::from([
                (Bytes::from(vec![1]), native_out),
                (Bytes::from(vec![2]), native_in),
                (Bytes::from(vec![3]), ok),
            ]))),
        );
        let admits_zero = Admission {
            tokens: BTreeSet::from([Address::ZERO, addr(1), addr(2)]),
            ..admission()
        };
        let p = IngestPipeline::new(
            norms,
            Duration::from_secs(60),
            1024,
            Arc::new(FixedClock(1000)),
            admits_zero,
        );
        let got = drain(&p, vec![feed(vec![raw(1), raw(2), raw(3)])]).await;
        assert_eq!(
            got.len(),
            1,
            "both native legs decline, the ERC20 order does not"
        );
        assert_eq!(got[0].id, IntentId(B256::from([3u8; 32])));
    }

    #[tokio::test]
    async fn drops_a_native_input_leg() {
        let mut native_in = intent(1, ChainId(1), 2000, 5, 5);
        native_in.input = IntentInput::new(Address::ZERO, AmountCurve::scalar(U256::from(5u64)));
        let map = HashMap::from([(Bytes::from(vec![1]), native_in)]);
        assert!(drain(&pipeline(map), vec![feed(vec![raw(1)])])
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn drops_unknown_protocol_and_normalize_errors() {
        // Empty normalizer set → no dispatch target; and a payload with no mapping → normalize error.
        let no_norms = IngestPipeline::new(
            BTreeMap::new(),
            Duration::from_secs(60),
            1024,
            Arc::new(FixedClock(1000)),
            admission(),
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
