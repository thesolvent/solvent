//! The pool read-surface: aggregates active strategies into pool views by composing the registry
//! snapshot (`pool_stats`) with the asset manager (labels, token metadata, stable classification).

use std::sync::Arc;

use crate::asset::AssetManager;
use crate::primitives::amount::{Amount, TokenAmount, TokenAmounts};
use crate::primitives::pool::{Pool, PoolDetail, PoolMaker, PoolType};
use crate::primitives::registry::{
    curve_label, fee_in_bps, CurveKind, CurveSpec, MakerStrategy, PoolStats, TokenPair,
};
use crate::registry::SharedSnapshot;

pub struct PoolService {
    registry: Arc<SharedSnapshot>,
    assets: Arc<AssetManager>,
}

impl PoolService {
    pub fn new(registry: Arc<SharedSnapshot>, assets: Arc<AssetManager>) -> Self {
        Self { registry, assets }
    }

    /// One row per active pair, registry-derived. `$`/volume fields are `None`/`0` until their data
    /// sources exist. A pair whose tokens are missing from the catalog is skipped (no metadata).
    pub fn pools(&self) -> Vec<Pool> {
        let snapshot = self.registry.load();
        snapshot
            .pool_stats()
            .into_iter()
            .filter_map(|(pair, stats)| self.assemble(&pair, &stats))
            .collect()
    }

    fn assemble(&self, pair: &TokenPair, stats: &PoolStats) -> Option<Pool> {
        let (base_addr, quote_addr) = self.assets.base_quote(pair);
        Some(Pool {
            pair: self.assets.pair_label(pair),
            base: self.assets.token(&base_addr)?,
            quote: self.assets.token(&quote_addr)?,
            pool_type: self.classify(pair, stats),
            maker_count: stats.maker_count as u64,
            min_spread_bps: stats.min_fee_bps,
            max_spread_bps: stats.max_fee_bps,
            popular_fee_tier: fee_tier(stats.popular_fee_bps),
            tvl_usd: None,
            volume_24h_usd: None,
            fills_24h: 0,
            apr_pct: None,
        })
    }

    /// Both tokens stable → `Stable`; else pegged-dominant → `Correlated`; else `Volatile`.
    fn classify(&self, pair: &TokenPair, stats: &PoolStats) -> PoolType {
        match (
            self.assets.is_stable(&pair.lo),
            self.assets.is_stable(&pair.hi),
            stats.curve_mix.dominant(),
        ) {
            (true, true, _) => PoolType::Stable,
            (_, _, Some(CurveKind::Pegged)) => PoolType::Correlated,
            _ => PoolType::Volatile,
        }
    }

    /// Full detail for `pair`, or `None` if it has no active pool. The row is the same as the list's;
    /// the roster is registry-derived (actual/pullable on-chain and recent fills join later).
    pub fn pool_detail(&self, pair: &TokenPair) -> Option<PoolDetail> {
        let snapshot = self.registry.load();
        let pool = self.assemble(pair, snapshot.pool_stats().get(pair)?)?;
        let makers = snapshot
            .active_strategies_for_pair(*pair)
            .filter_map(|strategy| self.pool_maker(strategy))
            .collect();
        Some(PoolDetail { pool, makers })
    }

    /// One roster entry: curve, fee, and committed balances. `None` for an unpriceable strategy.
    fn pool_maker(&self, strategy: &MakerStrategy) -> Option<PoolMaker> {
        let CurveSpec::Priceable { curve, fees_in_bps } = &strategy.curve else {
            return None;
        };
        let entries = strategy
            .balances
            .iter()
            .filter_map(|(address, balance)| {
                let token = self.assets.token(address)?;
                Some(TokenAmount {
                    amount: Amount::from_base_units(*balance, token.decimals),
                    token,
                })
            })
            .collect();
        Some(PoolMaker {
            maker: strategy.key.maker.0,
            strategy_hash: format!("{:#x}", strategy.key.strategy_hash.0),
            curve: curve_label(curve).to_string(),
            fee_bps: fee_in_bps(fees_in_bps),
            virtual_balances: TokenAmounts {
                entries,
                total_usd: None,
            },
        })
    }
}

/// Real bps → a percentage tier string, e.g. `5` → `"0.05%"`.
fn fee_tier(bps: u32) -> String {
    format!("{:.2}%", bps as f64 / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::asset::{TokenList, TokenMeta};
    use crate::primitives::registry::{
        Curve, CurveSpec, MakerStrategy, PeggedParams, Snapshot, StrategyKey,
    };
    use crate::primitives::{MakerId, StrategyHash};
    use alloy_primitives::{Address, B256, U256};

    fn addr(n: u8) -> Address {
        Address::from([n; 20])
    }

    fn meta(n: u8, symbol: &str, decimals: u8, stable: bool) -> TokenMeta {
        TokenMeta {
            chain_id: 31337,
            address: addr(n),
            symbol: symbol.to_string(),
            name: symbol.to_string(),
            decimals,
            logo_uri: None,
            tags: if stable {
                vec!["stables".to_string()]
            } else {
                vec![]
            },
        }
    }

    fn peg() -> PeggedParams {
        let one = U256::from(1u64);
        PeggedParams {
            x0: one,
            y0: one,
            linear_width: one,
            rate_lt: one,
            rate_gt: one,
        }
    }

    fn strat(s: u8, a: Address, b: Address, curve: Curve) -> MakerStrategy {
        let key = StrategyKey {
            maker: MakerId(Address::from([s; 20])),
            app: Address::ZERO,
            strategy_hash: StrategyHash(B256::from([s; 32])),
        };
        let mut st = MakerStrategy::new(key, &[]);
        st.curve = CurveSpec::Priceable {
            curve,
            fees_in_bps: vec![500_000],
        };
        st.balances.insert(a, U256::from(1u64));
        st.balances.insert(b, U256::from(1u64));
        st
    }

    #[test]
    fn classifies_pools_and_orders_base_quote() {
        let list = TokenList {
            name: "test".to_string(),
            tokens: vec![
                meta(1, "USDC", 6, true),
                meta(2, "USDT", 6, true),
                meta(3, "WETH", 18, false),
            ],
        };
        let snap = Snapshot::from_strategies([
            strat(0, addr(1), addr(2), Curve::Pegged(peg())), // USDC/USDT — both stable
            strat(1, addr(3), addr(1), Curve::Xyc),           // WETH/USDC
        ]);
        let registry = Arc::new(SharedSnapshot::new(snap));
        let assets = Arc::new(AssetManager::new(list, Arc::clone(&registry)));
        let pools = PoolService::new(registry, assets).pools();

        let stable = pools
            .iter()
            .find(|p| p.pool_type == PoolType::Stable)
            .unwrap();
        assert_eq!(stable.pair, "USDC/USDT");

        let volatile = pools
            .iter()
            .find(|p| p.pool_type == PoolType::Volatile)
            .unwrap();
        assert_eq!(volatile.pair, "WETH/USDC"); // stable is the quote
        assert_eq!(volatile.base.symbol, "WETH");
        assert_eq!(volatile.quote.symbol, "USDC");
        assert_eq!(volatile.popular_fee_tier, "0.05%");
    }

    #[test]
    fn pool_detail_has_roster_with_decimal_balances() {
        let list = TokenList {
            name: "test".to_string(),
            tokens: vec![meta(3, "WETH", 18, false), meta(1, "USDC", 6, true)],
        };
        let mut strategy = strat(1, addr(3), addr(1), Curve::Xyc);
        strategy
            .balances
            .insert(addr(3), U256::from(4_000_000_000_000_000_000u128)); // 4 WETH (18 dec)
        strategy
            .balances
            .insert(addr(1), U256::from(12_000_000_000u64)); // 12000 USDC (6 dec)
        let registry = Arc::new(SharedSnapshot::new(Snapshot::from_strategies([strategy])));
        let assets = Arc::new(AssetManager::new(list, Arc::clone(&registry)));
        let svc = PoolService::new(registry, assets);

        let detail = svc.pool_detail(&TokenPair::new(addr(3), addr(1))).unwrap();
        assert_eq!(detail.pool.pair, "WETH/USDC");
        assert_eq!(detail.pool.maker_count, 1);
        let maker = &detail.makers[0];
        assert_eq!(maker.curve, "XYC");
        let amount = |sym: &str| {
            maker
                .virtual_balances
                .entries
                .iter()
                .find(|e| e.token.symbol == sym)
                .unwrap()
                .amount
                .display
                .clone()
        };
        assert_eq!(amount("WETH"), "4");
        assert_eq!(amount("USDC"), "12000");

        // an unknown pair has no detail
        assert!(svc.pool_detail(&TokenPair::new(addr(1), addr(2))).is_none());
    }
}
