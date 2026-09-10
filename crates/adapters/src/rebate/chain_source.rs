//! Alloy log source for public rebate executions emitted by the configured filler.

#![allow(clippy::too_many_arguments)]

use alloy::sol;
use async_trait::async_trait;
use solvent_core::deps::rebate::{RebateChainSource, RebateChainSourceError};
use solvent_core::primitives::rebate::RebateExecutedEvent;
use solvent_core::primitives::{MakerId, RebateBatchId, StrategyHash};

use crate::{event_provider, events::prelude::*};

sol! {
    interface IRebateFiller {
        event RebateExecuted(
            bytes32 indexed contextHash,
            bytes32 indexed strategyHash,
            address indexed executor,
            address maker,
            address tokenIn,
            address tokenOut,
            uint256 amountIn,
            uint256 amountOut,
            uint256 rebateAmount
        );
    }
}

event_provider!(rebate_events, (filler, IRebateFiller::IRebateFillerEvents));

pub struct AlloyRebateChainSource {
    events: rebate_events::EventProvider,
    filler: Address,
}

impl AlloyRebateChainSource {
    pub fn new(provider: Arc<dyn Provider>, filler: Address, max_block_span: Option<u64>) -> Self {
        Self {
            events: rebate_events::EventProvider::new(provider, max_block_span, None),
            filler,
        }
    }
}

#[async_trait]
impl RebateChainSource for AlloyRebateChainSource {
    async fn fetch(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<RebateExecutedEvent>, RebateChainSourceError> {
        let (raw,) = self
            .events
            .query(
                from_block,
                to_block,
                &rebate_events::AddressFilter::new().filler(vec![self.filler]),
            )
            .await
            .map_err(|error| RebateChainSourceError::Rpc(error.to_string()))?;
        let mut events = raw
            .into_iter()
            .filter(|event| !event.removed)
            .map(into_domain)
            .collect::<Result<Vec<_>, _>>()?;
        events.sort_by_key(|event| (event.block_number, event.log_index));
        Ok(events)
    }
}

fn into_domain(
    event: solvent_core::primitives::registry::EventExt<IRebateFiller::IRebateFillerEvents>,
) -> Result<RebateExecutedEvent, RebateChainSourceError> {
    let block_number = event
        .block_number
        .ok_or(RebateChainSourceError::Unpositioned)?;
    let log_index = event
        .log_index
        .ok_or(RebateChainSourceError::Unpositioned)?;
    let tx_hash = event
        .transaction_hash
        .ok_or(RebateChainSourceError::Unpositioned)?;
    let IRebateFiller::IRebateFillerEvents::RebateExecuted(value) = event.event;
    Ok(RebateExecutedEvent::new(
        RebateBatchId(value.contextHash),
        StrategyHash(value.strategyHash),
        value.executor,
        MakerId(value.maker),
        value.tokenIn,
        value.tokenOut,
        value.amountIn,
        value.amountOut,
        value.rebateAmount,
        tx_hash,
        block_number,
        log_index,
    ))
}
