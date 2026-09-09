//! The normalizer and the admission rules against orders Uniswap actually produced.
//!
//! `tests/fixtures/mainnet/dutch_v2_orders.json` is a live capture from
//! `api.uniswap.org/v2/orders`, reduced to the smallest set covering every decode path we care
//! about: native-currency outputs, multiple outputs, a decaying input (exact-output orders), and
//! both exclusivity states. Each record carries the `orderHash` Uniswap's own service computed, so
//! these double as a contract-fidelity check on our `V2DutchOrderLib.hash` port — against real
//! orders rather than one generated fixture.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::{address, Address, Bytes, B256};
use futures::stream::{self, BoxStream};
use serde::Deserialize;
use solvent_adapters::ingest::uniswapx::UniswapXV2Normalizer;
use solvent_core::deps::ingest::{Normalizer, OrderFeed};
use solvent_core::deps::ledger::Clock;
use solvent_core::ingest::{Admission, IngestPipeline};
use solvent_core::primitives::ingest::{Intent, ProtocolId, RawOrder};
use solvent_core::primitives::{ChainId, IntentId};
use tokio::sync::mpsc;

const REACTOR: Address = address!("00000011F84B9aa48e5f8aA8B9897600006289Be");
const COSIGNER: Address = address!("4449Cd34d1eb1FEDCF02A1Be3834FfDe8E6A6180");
const NATIVE: Address = Address::ZERO;

#[derive(Deserialize)]
struct Corpus {
    orders: Vec<LiveOrder>,
}

/// One captured order, as the Orders API serves it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiveOrder {
    order_hash: String,
    encoded_order: String,
    signature: String,
    created_at: u64,
    facets: Vec<String>,
}

impl LiveOrder {
    fn raw(&self) -> RawOrder {
        RawOrder::new(
            ProtocolId::UniswapXV2,
            ChainId(1),
            Bytes::from_str(&self.encoded_order).expect("captured order is hex"),
            Bytes::from_str(&self.signature).expect("captured signature is hex"),
            self.created_at,
        )
    }

    fn has(&self, facet: &str) -> bool {
        self.facets.iter().any(|f| f == facet)
    }
}

fn corpus() -> Vec<LiveOrder> {
    let raw = include_str!("fixtures/mainnet/dutch_v2_orders.json");
    serde_json::from_str::<Corpus>(raw)
        .expect("corpus parses")
        .orders
}

fn normalizer() -> UniswapXV2Normalizer {
    UniswapXV2Normalizer::new(REACTOR, vec![COSIGNER])
}

/// Our `V2DutchOrderLib.hash` port against the hash Uniswap's own service published, for every
/// captured order. A single generated fixture cannot catch a field we mis-encode only when it is
/// populated; a live set spanning multi-output and decaying-input orders can.
#[test]
fn order_hash_matches_uniswaps_for_every_live_order() {
    let normalizer = normalizer();
    let orders = corpus();
    assert!(orders.len() >= 4, "corpus should cover every decode path");
    for order in &orders {
        let intent = normalizer
            .normalize(&order.raw())
            .unwrap_or_else(|e| panic!("live order {} rejected: {e}", order.order_hash));
        let expected = IntentId(B256::from_str(&order.order_hash).expect("hash is hex"));
        assert_eq!(intent.id, expected, "order {}", order.order_hash);
    }
}

/// Every captured order is cosigned by the live Uniswap key; a different allow-list rejects them
/// all, which is what pins the identity check rather than merely the signature check.
#[test]
fn a_foreign_cosigner_allow_list_rejects_the_whole_corpus() {
    let stranger = UniswapXV2Normalizer::new(REACTOR, vec![Address::repeat_byte(0x11)]);
    for order in corpus() {
        assert!(
            stranger.normalize(&order.raw()).is_err(),
            "order {} should not verify against a foreign cosigner",
            order.order_hash
        );
    }
}

/// The exclusivity toll is carried through decode, and an open order carries no window at all.
#[test]
fn exclusivity_survives_decode() {
    let normalizer = normalizer();
    for order in corpus() {
        let intent = normalizer.normalize(&order.raw()).expect("normalizes");
        match order.has("open") {
            true => assert!(
                intent.exclusivity.is_none(),
                "order {} is open",
                order.order_hash
            ),
            false => {
                let ex = intent
                    .exclusivity
                    .unwrap_or_else(|| panic!("order {} is exclusive", order.order_hash));
                assert_ne!(ex.filler, Address::ZERO);
                // Mainnet parameterizes V2 at 100 bps; a strict window would make the order
                // unfillable by anyone but its assignee, which the decision path must be able to
                // tell apart.
                assert_eq!(ex.override_bps, 100, "order {}", order.order_hash);
            }
        }
    }
}

/// Real orders exercise the shapes our synthetic fixtures never produce.
#[test]
fn decodes_the_shapes_synthetic_orders_miss() {
    let normalizer = normalizer();
    let orders = corpus();

    let multi = orders
        .iter()
        .find(|o| o.has("multi_output"))
        .expect("corpus has a multi-output order");
    let intent = normalizer.normalize(&multi.raw()).expect("normalizes");
    assert!(
        intent.outputs.len() > 1,
        "multi-output order decoded {} outputs",
        intent.outputs.len()
    );

    let decaying = orders
        .iter()
        .find(|o| o.has("decaying_input"))
        .expect("corpus has an exact-output order");
    let intent = normalizer.normalize(&decaying.raw()).expect("normalizes");
    let at_start = intent.input.curve.amount_at(0);
    let at_end = intent.input.curve.amount_at(u64::MAX);
    assert!(
        at_end > at_start,
        "an exact-output order's input decays upward: {at_start} -> {at_end}"
    );

    let native = orders
        .iter()
        .find(|o| o.has("native_output"))
        .expect("corpus has a native-output order");
    let intent = normalizer.normalize(&native.raw()).expect("normalizes");
    assert!(
        intent.outputs.iter().any(|o| o.token == NATIVE),
        "native output decoded as the zero address"
    );
}

/// A clock pinned inside the captured orders' lifetime. Without it every order is long expired and
/// admission drops the lot, which would make the rule under test unobservable.
struct FixedClock(u64);

impl Clock for FixedClock {
    fn now_unix(&self) -> u64 {
        self.0
    }
}

struct OneShot(Vec<RawOrder>);

impl OrderFeed for OneShot {
    fn stream(&self) -> BoxStream<'static, RawOrder> {
        Box::pin(stream::iter(self.0.clone()))
    }
}

/// The pipeline's verdict on the live corpus: a native-currency leg is declined at the door,
/// because the reactor pays it from its own balance and this filler settles ERC20 only. Everything
/// else whose tokens we admit gets through.
#[tokio::test]
async fn admission_declines_native_legs_from_the_live_corpus() {
    let orders = corpus();
    let normalizer = normalizer();

    // Admit every ERC20 the corpus touches, so the only rule under test is the native one, and
    // record which orders should survive.
    let mut tokens = BTreeSet::new();
    let mut expected: Vec<IntentId> = Vec::new();
    let mut earliest_deadline = u64::MAX;
    for order in &orders {
        let intent = normalizer.normalize(&order.raw()).expect("normalizes");
        earliest_deadline = earliest_deadline.min(intent.deadline);
        let touches_native =
            intent.input.token == NATIVE || intent.outputs.iter().any(|o| o.token == NATIVE);
        if touches_native {
            continue;
        }
        tokens.insert(intent.input.token);
        tokens.extend(intent.outputs.iter().map(|o| o.token));
        expected.push(intent.id);
    }
    assert!(
        !expected.is_empty() && expected.len() < orders.len(),
        "the corpus must contain both admissible and native orders"
    );

    let mut normalizers: BTreeMap<ProtocolId, Arc<dyn Normalizer>> = BTreeMap::new();
    normalizers.insert(ProtocolId::UniswapXV2, Arc::new(normalizer));
    let pipeline = IngestPipeline::new(
        normalizers,
        Duration::from_secs(60),
        1024,
        Arc::new(FixedClock(earliest_deadline - 1)),
        Admission {
            supported_chains: BTreeSet::from([ChainId(1)]),
            tokens,
            // Deliberately generous: the corpus's multi-output order must not be declined for its
            // output count while the native rule is under test.
            max_outputs: 8,
        },
    );

    let admitted = drain(&pipeline, orders.iter().map(|o| o.raw()).collect()).await;
    let admitted_ids: BTreeSet<IntentId> = admitted.iter().map(|i| i.id).collect();
    assert_eq!(
        admitted_ids,
        expected.into_iter().collect::<BTreeSet<_>>(),
        "exactly the non-native orders are admitted"
    );
}

async fn drain(pipeline: &IngestPipeline, raws: Vec<RawOrder>) -> Vec<Intent> {
    let (tx, mut rx) = mpsc::channel(64);
    pipeline.run(vec![Arc::new(OneShot(raws))], tx).await;
    let mut out = Vec::new();
    while let Some(intent) = rx.recv().await {
        out.push(intent);
    }
    out
}
