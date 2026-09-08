//! `AquaSettlementReader` — the UniswapX/Aqua [`SettlementReader`]: read the *actual* amounts a
//! confirmed fill pulled per source from the fill tx's own Aqua `Pulled` events. Reuses the shared
//! Aqua decode; the amounts it returns are what the ledger posts (not the conservative hold).

use std::sync::Arc;

use alloy::primitives::{Address, B256, U256};
use async_trait::async_trait;

use solvent_core::deps::execution::{SettlementError, SettlementReader};
use solvent_core::primitives::ledger::ReservationSource;
use solvent_core::primitives::registry::AquaEvent;

use crate::aqua::events_in_tx;
use crate::events::prelude::Provider;

/// Reads a confirmed fill's settlement from its receipt. `aqua` is the liquidity contract whose
/// `Pulled` events record what each maker gave up — the same contract the registry reads.
pub struct AquaSettlementReader {
    provider: Arc<dyn Provider>,
    aqua: Address,
}

impl AquaSettlementReader {
    pub fn new(provider: Arc<dyn Provider>, aqua: Address) -> Self {
        Self { provider, aqua }
    }
}

#[async_trait]
impl SettlementReader for AquaSettlementReader {
    async fn settled(
        &self,
        tx: B256,
        sources: &[ReservationSource],
    ) -> Result<Vec<U256>, SettlementError> {
        let pulls = events_in_tx(&self.provider, self.aqua, tx)
            .await
            .map_err(SettlementError::Read)?;

        Ok(sources.iter().map(|s| pulled_for(&pulls, s)).collect())
    }
}

/// Sum the amounts pulled for one source — the `Pulled` events matching its `(maker, strategy, token)`.
/// Zero if the fill never touched it. Only `Pulled` counts: a `Pushed` (the taker's inflow) on the
/// same key is the wrong direction and must not be booked as maker capital consumed.
fn pulled_for(pulls: &[AquaEvent], source: &ReservationSource) -> U256 {
    pulls
        .iter()
        .filter_map(|event| match event {
            AquaEvent::Pulled {
                maker,
                strategy_hash,
                token,
                amount,
                ..
            } if *maker == source.maker
                && *strategy_hash == source.strategy_hash
                && *token == source.token =>
            {
                Some(*amount)
            }
            _ => None,
        })
        .fold(U256::ZERO, |acc, amount| acc.saturating_add(amount))
}

#[cfg(test)]
mod tests {
    use super::*;
    use solvent_core::primitives::{MakerId, StrategyHash};

    fn source(maker: u8, token: u8, amount: u64) -> ReservationSource {
        ReservationSource {
            maker: MakerId(Address::from([maker; 20])),
            strategy_hash: StrategyHash(B256::from([1; 32])),
            token: Address::from([token; 20]),
            amount: U256::from(amount),
        }
    }
    fn pulled(s: &ReservationSource, amount: u64) -> AquaEvent {
        AquaEvent::Pulled {
            maker: s.maker,
            app: Address::ZERO,
            strategy_hash: s.strategy_hash,
            token: s.token,
            amount: U256::from(amount),
        }
    }

    #[test]
    fn sums_matching_pulls_and_zero_for_untouched() {
        let s = source(7, 9, 100);
        let pulls = vec![
            pulled(&s, 60),
            // wrong direction on the same key — must not count as consumed.
            AquaEvent::Pushed {
                maker: s.maker,
                app: Address::ZERO,
                strategy_hash: s.strategy_hash,
                token: s.token,
                amount: U256::from(999u64),
            },
        ];
        assert_eq!(pulled_for(&pulls, &s), U256::from(60u64));
        // A different maker's source the fill never pulled from → zero.
        assert_eq!(pulled_for(&pulls, &source(8, 9, 100)), U256::ZERO);
    }
}
