//! The settled-legs port: read a confirmed fill's *actual* per-leg amounts, so recapture values what
//! the fill really moved rather than the routing plan's expectation. One impl per protocol — the live
//! reader decodes the fill tx's own settlement events, the same source the ledger posts from.

use alloy_primitives::B256;
use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::routing::RouteLeg;

/// Reads a confirmed fill's actual per-leg amounts. Given the plan `legs` (for each leg's identity —
/// maker, strategy, and the two token sides), returns the same legs with `amount_in` / `amount_out`
/// overwritten by what the fill actually pushed to / pulled from that maker. A leg the fill never
/// touched comes back with zero amounts (and so recaptures nothing).
#[async_trait]
pub trait SettledLegsReader: Send + Sync {
    async fn actual_legs(
        &self,
        tx: B256,
        legs: &[RouteLeg],
    ) -> Result<Vec<RouteLeg>, SettledLegsError>;
}

/// A failure reading the settled legs (the receipt was unreachable or undecodable).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SettledLegsError {
    #[error("settled legs read: {0}")]
    Read(String),
}
