//! Dispatches routed intents to their protocol-specific filler.

use std::sync::Arc;

use async_trait::async_trait;
use solvent_core::deps::ingest::{FillBuilder, FillBuilderError, PreparedFill};
use solvent_core::primitives::ingest::{Intent, ProtocolId};
use solvent_core::primitives::registry::Snapshot;
use solvent_core::primitives::routing::RoutePlan;

pub struct ProtocolFillBuilder {
    uniswapx: Arc<dyn FillBuilder>,
    erc7683: Option<Arc<dyn FillBuilder>>,
    oneinch: Option<Arc<dyn FillBuilder>>,
}

impl ProtocolFillBuilder {
    pub fn new(
        uniswapx: Arc<dyn FillBuilder>,
        erc7683: Option<Arc<dyn FillBuilder>>,
        oneinch: Option<Arc<dyn FillBuilder>>,
    ) -> Self {
        Self {
            uniswapx,
            erc7683,
            oneinch,
        }
    }
}

#[async_trait]
impl FillBuilder for ProtocolFillBuilder {
    async fn build(
        &self,
        intent: &Intent,
        plan: &RoutePlan,
        snapshot: &Snapshot,
    ) -> Result<PreparedFill, FillBuilderError> {
        match intent.protocol {
            ProtocolId::UniswapXV2 => self.uniswapx.build(intent, plan, snapshot).await,
            ProtocolId::Erc7683 => match &self.erc7683 {
                Some(builder) => builder.build(intent, plan, snapshot).await,
                None => Err(FillBuilderError::UnsupportedProtocol),
            },
            ProtocolId::OneInchLimitOrder => match &self.oneinch {
                Some(builder) => builder.build(intent, plan, snapshot).await,
                None => Err(FillBuilderError::UnsupportedProtocol),
            },
            _ => Err(FillBuilderError::UnsupportedProtocol),
        }
    }
}
