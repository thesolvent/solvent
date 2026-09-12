//! The fill-builder port: turn a routed plan into the calldata that settles it on-chain. One impl
//! per protocol; the caller sends the returned calldata to that protocol's filler contract.

use alloy_primitives::{Address, Bytes};
use async_trait::async_trait;
use thiserror::Error;

use crate::deps::execution::ExecutionAuthorizerError;

use crate::primitives::ingest::Intent;
use crate::primitives::registry::Snapshot;
use crate::primitives::routing::RoutePlan;

/// A protocol-selected contract call ready for simulation and submission.
#[non_exhaustive]
pub struct PreparedFill {
    pub target: Address,
    pub calldata: Bytes,
}

impl PreparedFill {
    pub fn new(target: Address, calldata: Bytes) -> Self {
        Self { target, calldata }
    }
}

#[async_trait]
pub trait FillBuilder: Send + Sync {
    /// Builds the target and ABI-encoded calldata for a routed plan. `snapshot` resolves each leg's
    /// maker strategy (the on-chain order to source from).
    async fn build(
        &self,
        intent: &Intent,
        plan: &RoutePlan,
        snapshot: &Snapshot,
    ) -> Result<PreparedFill, FillBuilderError>;
}

/// A fill-build failure. All are unreachable on the normal route→fill path (a routed leg always has
/// a decodable strategy in the snapshot it was routed against), but must not panic.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FillBuilderError {
    #[error("plan has no legs to fill")]
    NoLegs,
    #[error("no strategy in the snapshot for a routed leg")]
    MissingStrategy,
    #[error("a routed leg's shipped program did not decode")]
    UndecodableProgram,
    #[error("a routed leg's maker does not match its shipped order")]
    StrategyMakerMismatch,
    #[error("a routed leg is not protected by the configured taker credential")]
    UnprotectedStrategy,
    #[error("the normalized order does not match this protocol filler")]
    InvalidOrder,
    #[error("no fill builder is configured for this protocol")]
    UnsupportedProtocol,
    #[error("a routed leg could not be policy-authorized: {0}")]
    Authorization(#[from] ExecutionAuthorizerError),
}
