use std::{collections::BTreeMap, ops::Range};

use alloy_primitives::{B256, U256};

use crate::deps::registry::{BlockTimes, BlockTimesError, StoreError};
use crate::primitives::amount::format_units;
use crate::primitives::asset::Token;
use crate::primitives::maker::{PositionHistory, StrategyPrice};
use crate::primitives::pricing::Ratio;
use crate::primitives::registry::{AquaEvent, Curve, CurveSpec, EventExt, MakerStrategy, Snapshot};
use crate::registry::CurvePool;
use crate::SolventError;

pub(super) fn marginal_price(
    strategy: &MakerStrategy,
    curve: &Curve,
    base: &Token,
    quote: &Token,
) -> Option<String> {
    if !strategy.active {
        return None;
    }
    let raw = CurvePool::from_curve(
        curve,
        base.address,
        quote.address,
        strategy.balance(&base.address),
        strategy.balance(&quote.address),
    )
    .marginal_price()
    .ok()?;
    let units = Ratio::new(
        U256::from(10).checked_pow(U256::from(base.decimals))?,
        U256::from(10).checked_pow(U256::from(quote.decimals))?,
    )?;
    let scaled = (raw * units * Ratio::from(U256::from(10).pow(U256::from(18)))).floor()?;
    (!scaled.is_zero()).then(|| format_units(scaled, 18))
}

// Earlier states are carried to the window edge; their individual timestamps are unnecessary.
pub(super) async fn window_times(
    reader: &dyn BlockTimes,
    hashes: &[B256],
    start: u64,
) -> Result<BTreeMap<B256, u64>, SolventError> {
    let mut left = 0;
    let mut right = hashes.len();
    while left < right {
        let middle = left + (right - left) / 2;
        let hash = hashes[middle];
        let times = reader.timestamps(&[hash]).await?;
        let at = times.get(&hash).ok_or(BlockTimesError::Missing(hash))?;
        if *at < start {
            left = middle + 1;
        } else {
            right = middle;
        }
    }
    let mut times = reader.timestamps(&hashes[left..]).await?;
    times.extend(hashes[..left].iter().map(|hash| (*hash, start)));
    Ok(times)
}

pub(super) fn price_history(
    strategy: &MakerStrategy,
    base: &Token,
    quote: &Token,
    events: &[EventExt<AquaEvent>],
    times: &BTreeMap<B256, u64>,
    window: Range<u64>,
) -> Result<PositionHistory, SolventError> {
    let mut result = PositionHistory {
        from: window.start,
        to: window.end,
        created_block: None,
        prices: Vec::new(),
    };
    let CurveSpec::Priceable { curve, .. } = &strategy.curve else {
        return Ok(result);
    };
    let mut snapshot = Snapshot::default();
    // A swap changes both reserves. Sampling between its logs would invent a tradable price.
    for transaction in events
        .chunk_by(|a, b| a.block_hash == b.block_hash && a.transaction_hash == b.transaction_hash)
    {
        let Some(first) = transaction.first() else {
            continue;
        };
        let hash = first.block_hash.ok_or(StoreError::Unpositioned)?;
        let at = *times.get(&hash).ok_or(BlockTimesError::Missing(hash))?;
        if at >= window.end {
            break;
        }
        for event in transaction.iter().filter(|e| !e.removed) {
            if matches!(event.event, AquaEvent::Shipped { .. }) && result.created_block.is_none() {
                result.created_block = event.block_number;
            }
            snapshot.apply(event.event.clone());
        }
        let price = snapshot
            .strategy(&strategy.key)
            .and_then(|s| marginal_price(s, curve, base, quote));
        let point = StrategyPrice {
            at: at.max(window.start),
            price,
        };
        if result.prices.last().is_some_and(|p| p.at == point.at) {
            result.prices.pop();
        }
        result.prices.push(point);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::registry::{PeggedParams, StrategyKey};
    use crate::primitives::{MakerId, StrategyHash};
    use alloy_primitives::{Address, Bytes};

    fn token(n: u8, decimals: u8) -> Token {
        Token {
            address: Address::from([n; 20]),
            decimals,
            chain_id: 1,
            symbol: n.to_string(),
            logo_uri: None,
        }
    }
    fn strategy() -> MakerStrategy {
        let mut strategy = MakerStrategy::new(
            StrategyKey {
                maker: MakerId(Address::ZERO),
                app: Address::ZERO,
                strategy_hash: StrategyHash(B256::ZERO),
            },
            &[],
        );
        strategy.curve = CurveSpec::Priceable {
            curve: Curve::Xyc,
            fees_in_bps: vec![500_000],
        };
        strategy
    }
    fn e18(n: u64) -> U256 {
        U256::from(n) * U256::from(10).pow(U256::from(18))
    }

    #[test]
    fn marginal_prices_use_oriented_committed_reserves_and_curve_derivatives() {
        let mut st = strategy();
        let base = token(1, 18);
        let quote = token(2, 6);
        st.balances = BTreeMap::from([
            (base.address, e18(2)),
            (quote.address, U256::from(6_000_000_000u64)),
        ]);
        assert_eq!(
            marginal_price(&st, &Curve::Xyc, &base, &quote).as_deref(),
            Some("3000")
        );
        st.balances.insert(base.address, e18(4));
        st.balances.insert(quote.address, U256::from(3_000_000));
        let pegged = Curve::Pegged(PeggedParams {
            x0: e18(1),
            y0: e18(3),
            linear_width: U256::from(10).pow(U256::from(27)),
            rate_lt: U256::from(1),
            rate_gt: U256::from(10).pow(U256::from(12)),
        });
        assert_eq!(
            marginal_price(&st, &pegged, &base, &quote).as_deref(),
            Some("2.5")
        );
        assert_eq!(
            marginal_price(&st, &pegged, &quote, &base).as_deref(),
            Some("0.4")
        );
        let quote = token(2, 18);
        let concentrated = Curve::Concentrate {
            sqrt_price_min: e18(1),
            sqrt_price_max: e18(2),
        };
        for (x, y, expected) in [(1, 3, "2.25"), (0, 6, "4"), (3, 0, "1")] {
            st.balances = BTreeMap::from([(base.address, e18(x)), (quote.address, e18(y))]);
            assert_eq!(
                marginal_price(&st, &concentrated, &base, &quote).as_deref(),
                Some(expected)
            );
        }
        st.balances = BTreeMap::from([
            (base.address, U256::from(10).pow(U256::from(30))),
            (quote.address, U256::from(10).pow(U256::from(10))),
        ]);
        assert_eq!(
            marginal_price(&st, &Curve::Xyc, &base, &quote),
            None,
            "positive sub-display prices must not become zero"
        );
        st.balances.clear();
        for curve in [Curve::Xyc, concentrated, pegged] {
            assert_eq!(marginal_price(&st, &curve, &base, &quote), None);
        }
        st.active = false;
        assert_eq!(marginal_price(&st, &concentrated, &base, &quote), None);
    }

    #[test]
    fn price_history_samples_complete_transactions_and_preserves_gaps() {
        let st = strategy();
        let StrategyKey {
            maker,
            app,
            strategy_hash,
        } = st.key;
        let base = token(1, 18);
        let quote = token(2, 18);
        let push = |token, amount| AquaEvent::Pushed {
            maker,
            app,
            strategy_hash,
            token,
            amount: e18(amount),
        };
        let rows = [
            (
                10,
                1,
                AquaEvent::Shipped {
                    maker,
                    app,
                    strategy_hash,
                    strategy: Bytes::new(),
                },
            ),
            (10, 1, push(base.address, 2)),
            (10, 1, push(quote.address, 6)),
            (20, 2, push(base.address, 1)),
            (
                20,
                2,
                AquaEvent::Pulled {
                    maker,
                    app,
                    strategy_hash,
                    token: quote.address,
                    amount: e18(3),
                },
            ),
            (
                30,
                3,
                AquaEvent::Docked {
                    maker,
                    app,
                    strategy_hash,
                },
            ),
        ];
        let events = rows
            .into_iter()
            .enumerate()
            .map(|(index, (block, tx, event))| EventExt {
                event,
                address: Address::ZERO,
                block_hash: Some(B256::from([block; 32])),
                block_number: Some(u64::from(block)),
                transaction_hash: Some(B256::from([tx; 32])),
                transaction_index: Some(0),
                log_index: Some(index as u64),
                removed: false,
            })
            .collect::<Vec<_>>();
        let times = BTreeMap::from([
            (B256::from([10; 32]), 100),
            (B256::from([20; 32]), 200),
            (B256::from([30; 32]), 300),
        ]);
        let result = price_history(&st, &base, &quote, &events, &times, 150..400).unwrap();
        assert_eq!(result.created_block, Some(10));
        assert_eq!(
            result
                .prices
                .iter()
                .map(|p| (p.at, p.price.as_deref()))
                .collect::<Vec<_>>(),
            vec![(150, Some("3")), (200, Some("1")), (300, None)]
        );
        let result = price_history(&st, &base, &quote, &events, &times, 50..400).unwrap();
        assert_eq!(
            result.prices[0].at, 100,
            "no fabricated history before shipping"
        );
        assert!(price_history(&st, &base, &quote, &events, &BTreeMap::new(), 50..400).is_err());
    }
    #[tokio::test]
    async fn history_header_reads_skip_ancient_blocks() {
        struct Times {
            reads: std::sync::atomic::AtomicUsize,
            times: BTreeMap<B256, u64>,
        }
        #[async_trait::async_trait]
        impl BlockTimes for Times {
            async fn timestamps(
                &self,
                hashes: &[B256],
            ) -> Result<BTreeMap<B256, u64>, BlockTimesError> {
                self.reads
                    .fetch_add(hashes.len(), std::sync::atomic::Ordering::Relaxed);
                hashes
                    .iter()
                    .map(|hash| {
                        self.times
                            .get(hash)
                            .map(|at| (*hash, *at))
                            .ok_or(BlockTimesError::Missing(*hash))
                    })
                    .collect()
            }
        }
        let hashes = (0..1024u64)
            .map(|i| B256::from(U256::from(i).to_be_bytes::<32>()))
            .collect::<Vec<_>>();
        let reader = Times {
            reads: Default::default(),
            times: hashes
                .iter()
                .enumerate()
                .map(|(i, hash)| (*hash, i as u64))
                .collect(),
        };
        let times = window_times(&reader, &hashes, 1020).await.unwrap();
        assert!(reader.reads.load(std::sync::atomic::Ordering::Relaxed) <= 15);
        assert_eq!(times[&hashes[0]], 1020);
        assert_eq!(times[&hashes[1023]], 1023);
    }
}
