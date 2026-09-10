//! The order-log port: record every order the feed shows us, whatever becomes of it.
//!
//! The trade store only holds orders this resolver acted on. Most of what arrives is refused at the
//! door — a native-currency leg, a token we do not make a market in — and leaves no trace at all,
//! so "we filled 3 of 4" reads as a 75% hit rate when the real denominator was several hundred.
//! This is the denominator. Best-effort telemetry: a failure here never affects ingest.

use async_trait::async_trait;
use thiserror::Error;

use crate::primitives::ingest::{Intent, OrderSource};
use crate::primitives::IntentId;

/// What became of an order between arriving and being acted on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OrderVerdict {
    /// Passed the door and reached the decision loop.
    Admitted,
    /// Refused before routing; the reason is the admission rule that refused it.
    Dropped(&'static str),
}

impl OrderVerdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            OrderVerdict::Admitted => "admitted",
            OrderVerdict::Dropped(_) => "dropped",
        }
    }

    /// The refusing rule, for a drop.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            OrderVerdict::Admitted => None,
            OrderVerdict::Dropped(reason) => Some(reason),
        }
    }
}

/// One order as the feed delivered it, with the verdict at the door. Amounts are priced at
/// `seen_at`, which is the only instant at which every order is comparable.
pub struct OrderObserved<'a> {
    pub intent: &'a Intent,
    pub source: OrderSource,
    pub verdict: OrderVerdict,
    pub seen_at: u64,
}

#[async_trait]
pub trait OrderLog: Send + Sync {
    /// Persist one observation. Best-effort — callers log and continue on error.
    async fn record(&self, order: &OrderObserved<'_>) -> Result<(), OrderLogError>;
    /// Record what sourcing the delivery would cost, once routing has priced it. Separate from
    /// `record` because the price is only known after admission, and it is what turns "refused"
    /// into "refused, and by how much".
    async fn record_quote(&self, order: IntentId, indicative_in: &str)
        -> Result<(), OrderLogError>;
}

/// An order-log failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OrderLogError {
    /// The database call failed.
    #[error("db: {0}")]
    Db(String),
}
