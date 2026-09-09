use std::sync::Arc;

use alloy_primitives::Address;

use crate::deps::asset::PairPriceHistorySource;
use crate::primitives::asset::{PairPriceHistory, PriceHistoryPeriod};
use crate::SolventError;

/// Historical pair prices for charting, isolated from the live oracle used by routing.
pub struct PairHistoryService {
    source: Arc<dyn PairPriceHistorySource>,
}

impl PairHistoryService {
    pub fn new(source: Arc<dyn PairPriceHistorySource>) -> Self {
        Self { source }
    }

    pub async fn history(
        &self,
        base: Address,
        quote: Address,
        period: PriceHistoryPeriod,
    ) -> Result<PairPriceHistory, SolventError> {
        if base == quote {
            return Err(SolventError::InvalidId {
                id_type: "pair",
                reason: "base and quote must differ".to_string(),
            });
        }
        Ok(self.source.history(base, quote, period).await?)
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::deps::asset::PairPriceHistorySourceError;

    struct FakeSource;

    #[async_trait]
    impl PairPriceHistorySource for FakeSource {
        async fn history(
            &self,
            base: Address,
            quote: Address,
            period: PriceHistoryPeriod,
        ) -> Result<PairPriceHistory, PairPriceHistorySourceError> {
            Ok(PairPriceHistory {
                base,
                quote,
                period,
                points: Vec::new(),
            })
        }
    }

    #[tokio::test]
    async fn rejects_a_pair_with_the_same_token_on_both_sides() {
        let token = Address::from([1; 20]);
        let service = PairHistoryService::new(Arc::new(FakeSource));

        let error = service
            .history(token, token, PriceHistoryPeriod::SevenDays)
            .await
            .expect_err("same-token pair must fail");

        assert!(matches!(error, SolventError::InvalidId { .. }));
    }
}
