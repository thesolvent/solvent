//! The fill-builder port: turn a routed plan into the calldata that settles it on-chain, and the
//! contract it must be sent to. One impl per protocol, each targeting its own deployed filler
//! contract — the target travels with the calldata rather than being assumed by the caller, since
//! two protocols never share one filler contract.

use alloy_primitives::{Address, Bytes};
use async_trait::async_trait;
use thiserror::Error;

use crate::deps::execution::ExecutionAuthorizerError;

use crate::primitives::ingest::Intent;
use crate::primitives::registry::Snapshot;
use crate::primitives::routing::RoutePlan;

/// One protocol's fill, ready to submit: the calldata, and the filler contract it targets.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BuiltFill {
    pub target: Address,
    pub calldata: Bytes,
}

impl BuiltFill {
    pub fn new(target: Address, calldata: Bytes) -> BuiltFill {
        BuiltFill { target, calldata }
    }
}

/// Builds the ABI-encoded fill calldata for a routed plan, and names the contract it targets.
/// `snapshot` resolves each leg's maker strategy (the on-chain order to source from).
#[async_trait]
pub trait FillBuilder: Send + Sync {
    async fn build(
        &self,
        intent: &Intent,
        plan: &RoutePlan,
        snapshot: &Snapshot,
    ) -> Result<BuiltFill, FillBuilderError>;
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
    #[error("intent raw/signature did not decode: {0}")]
    MalformedIntent(&'static str),
    #[error("a routed leg's maker does not match its shipped order")]
    StrategyMakerMismatch,
    #[error("a routed leg is not protected by the configured taker credential")]
    UnprotectedStrategy,
    #[error("a routed leg could not be policy-authorized: {0}")]
    Authorization(#[from] ExecutionAuthorizerError),
}
