//! The fill-builder port: turn a routed plan into the calldata that settles it on-chain. One impl
//! per protocol; the caller sends the returned calldata to that protocol's filler contract.

use alloy_primitives::Bytes;
use thiserror::Error;

use crate::primitives::ingest::Intent;
use crate::primitives::registry::Snapshot;
use crate::primitives::routing::RoutePlan;

/// Builds the ABI-encoded fill calldata for a routed plan. `snapshot` resolves each leg's maker
/// strategy (the on-chain order to source from).
pub trait FillBuilder: Send + Sync {
    fn build(
        &self,
        intent: &Intent,
        plan: &RoutePlan,
        snapshot: &Snapshot,
    ) -> Result<Bytes, FillBuilderError>;
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
}
