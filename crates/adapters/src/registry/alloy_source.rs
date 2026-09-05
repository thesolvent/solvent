//! The [`ChainSource`] adapter backed by alloy's `eth_getLogs`.
//!
//! It wires the vendored batched indexer to the Aqua event ABI: one `getLogs`
//! sweep over `[from, to]` scoped to the Aqua contract, decoded into domain
//! [`AquaEvent`]s. Only strategies on our configured `app` (SwapVM router) are
//! kept — one Aqua deployment serves many routers.

use solvent_core::{
    deps::registry::{ChainSource, ChainSourceError},
    primitives::registry::{AquaEvent, EventExt},
};

use crate::aqua::{into_domain, IAqua};
use crate::{event_provider, events::prelude::*};
use async_trait::async_trait;

event_provider!(aqua_events, (aqua, IAqua::IAquaEvents));

/// Reads Aqua events from an EVM node via batched `eth_getLogs`.
pub struct AlloyChainSource {
    events: aqua_events::EventProvider,
    aqua: Address,
    app: Address,
}

impl AlloyChainSource {
    /// `aqua` is the liquidity contract to read logs from; `app` is the SwapVM
    /// router whose strategies we keep. `max_block_span` caps the blocks per
    /// `getLogs` chunk (the indexer default when `None`).
    pub fn new(
        provider: Arc<dyn Provider>,
        aqua: Address,
        app: Address,
        max_block_span: Option<u64>,
    ) -> Self {
        let events = aqua_events::EventProvider::new(provider, max_block_span, None);
        Self { events, aqua, app }
    }
}

#[async_trait]
impl ChainSource for AlloyChainSource {
    async fn fetch(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<EventExt<AquaEvent>>, ChainSourceError> {
        let (raw,) = self
            .events
            .query(
                from_block,
                to_block,
                &aqua_events::AddressFilter::new().aqua(vec![self.aqua]),
            )
            .await
            .map_err(|e| ChainSourceError::Rpc(e.to_string()))?;

        let mut events: Vec<EventExt<AquaEvent>> = raw
            .into_iter()
            .map(|ext| ext.map_event(into_domain))
            .filter(|ext| ext.event.key().app == self.app)
            .collect();

        events.sort_by_key(|e| (e.block_number, e.log_index));
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::prelude::process_logs;
    use alloy::primitives::{Bytes, LogData, B256, U256};
    use alloy::rpc::types::Log;
    use solvent_core::primitives::{MakerId, StrategyHash};

    /// The fixture written by the study lab's `gen-logs` test: real Aqua logs
    /// from a ship+swap+dock against the deployed contract on anvil.
    #[derive(serde::Deserialize)]
    struct Fixture {
        aqua: Address,
        app: Address,
        maker: Address,
        #[serde(rename = "tokenA")]
        token_a: Address,
        #[serde(rename = "tokenB")]
        token_b: Address,
        #[serde(rename = "strategyHash")]
        strategy_hash: B256,
        logs: Vec<RawLog>,
    }

    #[derive(serde::Deserialize)]
    struct RawLog {
        address: Address,
        topics: Vec<B256>,
        data: Bytes,
        #[serde(rename = "blockNumber")]
        block_number: u64,
        #[serde(rename = "blockHash")]
        block_hash: B256,
        #[serde(rename = "logIndex")]
        log_index: u64,
    }

    // Build the alloy rpc `Log` by hand — the fixture stores block/log numbers as
    // decimals, which alloy's own hex-quantity `Deserialize` would reject.
    fn rpc_log(r: &RawLog) -> Log {
        Log {
            inner: alloy::primitives::Log {
                address: r.address,
                data: LogData::new_unchecked(r.topics.clone(), r.data.clone()),
            },
            block_hash: Some(r.block_hash),
            block_number: Some(r.block_number),
            block_timestamp: None,
            transaction_hash: None,
            transaction_index: None,
            log_index: Some(r.log_index),
            removed: false,
        }
    }

    // Runs the real logs through the exact decode path `fetch` uses after
    // `get_logs`: our `sol! IAqua` topic0 hashes and un-indexed layout must match
    // the deployed contract byte-for-byte, and `into_domain` must land every field.
    #[test]
    fn decodes_real_aqua_logs_into_domain_events() {
        let fx: Fixture = serde_json::from_str(include_str!("../../tests/fixtures/aqua_logs.json"))
            .expect("fixture parses");
        let logs: Vec<Log> = fx.logs.iter().map(rpc_log).collect();

        let events: Vec<AquaEvent> = process_logs::<IAqua::IAquaEvents>(&logs, &[fx.aqua])
            .into_iter()
            .map(|ext| into_domain(ext.event))
            .collect();

        // All six decode — zero would mean a topic0 signature mismatch.
        assert_eq!(events.len(), 6);

        let e18 = U256::from(10u64).pow(U256::from(18u64));
        let key_fields = (MakerId(fx.maker), fx.app, StrategyHash(fx.strategy_hash));

        match &events[0] {
            AquaEvent::Shipped {
                maker,
                app,
                strategy_hash,
                strategy,
            } => {
                assert_eq!((*maker, *app, *strategy_hash), key_fields);
                assert!(!strategy.is_empty(), "shipped strategy bytes present");
            }
            other => panic!("expected Shipped, got {other:?}"),
        }

        // ship legs: 1000e18 of each token; swap in-leg: taker pushes 100e18 tokenB.
        assert_eq!(
            events[1],
            AquaEvent::Pushed {
                maker: key_fields.0,
                app: key_fields.1,
                strategy_hash: key_fields.2,
                token: fx.token_a,
                amount: e18 * U256::from(1000u64),
            }
        );
        assert_eq!(
            events[2],
            AquaEvent::Pushed {
                maker: key_fields.0,
                app: key_fields.1,
                strategy_hash: key_fields.2,
                token: fx.token_b,
                amount: e18 * U256::from(1000u64),
            }
        );
        assert_eq!(
            events[3],
            AquaEvent::Pushed {
                maker: key_fields.0,
                app: key_fields.1,
                strategy_hash: key_fields.2,
                token: fx.token_b,
                amount: e18 * U256::from(100u64),
            }
        );

        // swap out-leg: maker pulls tokenA (the XYC output, > 0).
        match &events[4] {
            AquaEvent::Pulled {
                maker,
                app,
                strategy_hash,
                token,
                amount,
            } => {
                assert_eq!((*maker, *app, *strategy_hash), key_fields);
                assert_eq!(*token, fx.token_a);
                assert!(*amount > U256::ZERO);
            }
            other => panic!("expected Pulled, got {other:?}"),
        }

        match &events[5] {
            AquaEvent::Docked {
                maker,
                app,
                strategy_hash,
            } => assert_eq!((*maker, *app, *strategy_hash), key_fields),
            other => panic!("expected Docked, got {other:?}"),
        }
    }
}
