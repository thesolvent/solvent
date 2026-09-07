//! ERC-7683 ingest against the unmodified `IngestPipeline`: a 7683 feed and a UniswapX feed fanned
//! into one pipeline must each yield their protocol's `Intent`, with no change to the pipeline
//! itself. That is the whole claim of the protocol-agnostic seam.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::{address, Address, B256, U256};
use alloy::signers::local::PrivateKeySigner;
use tokio::sync::mpsc;

use solvent_adapters::ingest::{erc7683, uniswapx};
use solvent_core::deps::ingest::{Normalizer, OrderFeed};
use solvent_core::deps::ledger::Clock;
use solvent_core::ingest::IngestPipeline;
use solvent_core::primitives::ingest::{Intent, ProtocolId};
use solvent_core::primitives::ChainId;

const PERMIT2: Address = address!("000000000022D473030F116dDEE9F6B43aC78BA3");
const CHAIN: u64 = 1;
const NOW: u64 = 1000;

struct FixedClock(u64);
impl Clock for FixedClock {
    fn now_unix(&self) -> u64 {
        self.0
    }
}

fn signer(byte: u8) -> PrivateKeySigner {
    PrivateKeySigner::from_bytes(&B256::from([byte; 32])).expect("valid test key")
}

fn erc7683_feed() -> Arc<dyn OrderFeed> {
    let builder = erc7683::SignedOrderBuilder::new(PERMIT2, CHAIN, signer(0x11));
    let spec = erc7683::OrderSpec {
        settler: address!("2222222222222222222222222222222222222222"),
        nonce: U256::from(42u64),
        open_deadline: 1500,
        fill_deadline: 2000,
        input_token: address!("6666666666666666666666666666666666666666"),
        input_amount: U256::from(500u64),
        output_token: address!("7777777777777777777777777777777777777777"),
        output_amount: U256::from(1000u64),
        recipient: address!("8888888888888888888888888888888888888888"),
        exclusive_filler: Address::ZERO,
        exclusivity_ends: 0,
    };
    Arc::new(erc7683::SelfHostedFeed::new(&builder, &[spec], NOW))
}

fn uniswapx_feed() -> Arc<dyn OrderFeed> {
    let builder = uniswapx::SignedOrderBuilder::new(PERMIT2, CHAIN, signer(0x21), signer(0x22));
    let spec = uniswapx::OrderSpec {
        reactor: address!("2222222222222222222222222222222222222222"),
        nonce: U256::from(7u64),
        deadline: 2000,
        input_token: address!("6666666666666666666666666666666666666666"),
        input_start: U256::from(500u64),
        input_end: U256::from(500u64),
        output_token: address!("7777777777777777777777777777777777777777"),
        output_start: U256::from(1000u64),
        output_end: U256::from(900u64),
        recipient: address!("8888888888888888888888888888888888888888"),
        decay_start: NOW,
        decay_end: NOW + 100,
        exclusive_filler: Address::ZERO,
    };
    Arc::new(uniswapx::SelfHostedFeed::new(&builder, &[spec], NOW))
}

fn pipeline() -> IngestPipeline {
    let mut normalizers: BTreeMap<ProtocolId, Arc<dyn Normalizer>> = BTreeMap::new();
    normalizers.insert(
        ProtocolId::UniswapXV2,
        Arc::new(uniswapx::UniswapXV2Normalizer),
    );
    normalizers.insert(ProtocolId::Erc7683, Arc::new(erc7683::Erc7683Normalizer));
    IngestPipeline::new(
        normalizers,
        Duration::from_secs(60),
        Arc::new(FixedClock(NOW)),
        BTreeSet::from([ChainId(CHAIN)]),
    )
}

async fn drain(feeds: Vec<Arc<dyn OrderFeed>>) -> Vec<Intent> {
    let (tx, mut rx) = mpsc::channel(64);
    pipeline().run(feeds, tx).await;
    let mut got = Vec::new();
    while let Some(i) = rx.recv().await {
        got.push(i);
    }
    got
}

#[tokio::test]
async fn one_pipeline_admits_both_protocols() {
    let got = drain(vec![erc7683_feed(), uniswapx_feed()]).await;

    let mut protocols: Vec<ProtocolId> = got.iter().map(|i| i.protocol).collect();
    protocols.sort();
    assert_eq!(
        protocols,
        vec![ProtocolId::UniswapXV2, ProtocolId::Erc7683],
        "both feeds should survive normalization and admission"
    );
}

#[tokio::test]
async fn a_seven_six_eight_three_order_survives_on_its_own() {
    let got = drain(vec![erc7683_feed()]).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].protocol, ProtocolId::Erc7683);
    assert_eq!(got[0].deadline, 2000);
    assert_eq!(got[0].origin_chain, ChainId(CHAIN));
}

