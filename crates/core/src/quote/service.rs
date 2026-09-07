//! The quote read-path: route an intent read-only over the live registry and the synced caps,
//! returning the split as a [`QuoteResponse`] — no reservation, no persistence. It reuses the swap
//! path's `select`/`solve_sparse` at the same per-leg gas cost (all read lock-free, no per-request
//! RPC), so a quote matches what the swap would execute.

use std::collections::BTreeMap;
use std::sync::Arc;

use alloy_primitives::{keccak256, Address, B256, U256};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::asset::AssetManager;
use crate::deps::ledger::Clock;
use crate::ledger::LedgerService;
use crate::primitives::amount::share_pct;
use crate::primitives::quote::{QuoteLeg, QuoteResponse};
use crate::primitives::registry::{curve_label, CurveSpec, Snapshot, TokenPair};
use crate::primitives::routing::{RouteLeg, RouteRequest, RoutingConfig};
use crate::primitives::{IntentId, StrategyHash};
use crate::registry::SharedSnapshot;
use crate::routing::{price_impact_pct, select, solve_sparse, LegCostResolver};
use crate::valuation::Valuation;

/// How far ahead a quote's advisory `expires_at` sits.
const QUOTE_TTL_SECS: u64 = 30;

pub struct QuoteService {
    registry: Arc<SharedSnapshot>,
    ledger: Arc<LedgerService>,
    assets: Arc<AssetManager>,
    config: RoutingConfig,
    clock: Arc<dyn Clock>,
    leg_cost: Arc<LegCostResolver>,
    valuation: Arc<Valuation>,
}

impl QuoteService {
    pub fn new(
        registry: Arc<SharedSnapshot>,
        ledger: Arc<LedgerService>,
        assets: Arc<AssetManager>,
        config: RoutingConfig,
        clock: Arc<dyn Clock>,
        leg_cost: Arc<LegCostResolver>,
        valuation: Arc<Valuation>,
    ) -> Self {
        Self {
            registry,
            ledger,
            assets,
            config,
            clock,
            leg_cost,
            valuation,
        }
    }

    /// A read-only quote for `amount_in` of `token_in` into `token_out`, or `None` when no route
    /// exists (no makers, or the size is beyond the book). Async only to read the gas/price cache;
    /// it does no I/O.
    pub async fn quote(
        &self,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
    ) -> Option<QuoteResponse> {
        let id = quote_hash(token_in, token_out, amount_in);
        let request = RouteRequest {
            intent: IntentId(id),
            token_in,
            token_out,
            amount: amount_in,
            exact_in: true,
        };
        let snapshot = self.registry.load();
        let caps = self.ledger.snapshot();

        // The same two steps the swap path runs: funnel, then the gas-aware split. `select` freezes
        // the caps; `solve_sparse` splits under the per-leg gas cost, capped at `max_legs`.
        let selection = select(&snapshot, &caps, &request, self.config.max_candidates);
        let out_decimals = self.assets.decimals(&token_out);
        let per_leg_cost = self.leg_cost.for_request(&request).await;
        let split = solve_sparse(
            &selection.chosen,
            &request,
            per_leg_cost,
            self.config.max_legs,
            None,
        )?;

        let labels = curve_labels(&snapshot, token_in, token_out);
        let mut legs = Vec::with_capacity(split.legs.len());
        for leg in &split.legs {
            if let Some(resolved) = self.leg(leg, &labels, split.amount_out).await {
                legs.push(resolved);
            }
        }

        Some(QuoteResponse {
            quote_id: format!("{id:#x}"),
            amount_out: self
                .valuation
                .amount(split.amount_out, token_out, out_decimals)
                .await,
            price_impact_pct: price_impact_pct(&selection.chosen, amount_in, split.amount_out),
            makers_sourced: split.legs.len() as u32,
            legs,
            expires_at: self.expires_at(),
        })
    }

    /// Resolve one routed leg to its wire shape, or skip it if a token is missing from the catalog.
    async fn leg(
        &self,
        leg: &RouteLeg,
        labels: &BTreeMap<StrategyHash, &'static str>,
        total_out: U256,
    ) -> Option<QuoteLeg> {
        let token_in = self.assets.token(&leg.token_in)?;
        let token_out = self.assets.token(&leg.token_out)?;
        Some(QuoteLeg {
            maker: leg.maker.0,
            strategy_hash: format!("{:#x}", leg.strategy_hash.0),
            amount_in: self
                .valuation
                .amount(leg.amount_in, token_in.address, token_in.decimals)
                .await,
            amount_out: self
                .valuation
                .amount(leg.amount_out, token_out.address, token_out.decimals)
                .await,
            curve: labels
                .get(&leg.strategy_hash)
                .copied()
                .unwrap_or_default()
                .to_string(),
            share_pct: share_pct(leg.amount_out, total_out),
            token_in,
            token_out,
        })
    }

    /// `now + TTL` as RFC-3339. A near-future unix second is always a valid, formattable instant.
    fn expires_at(&self) -> String {
        let secs = self.clock.now_unix().saturating_add(QUOTE_TTL_SECS);
        OffsetDateTime::from_unix_timestamp(secs as i64)
            .ok()
            .and_then(|instant| instant.format(&Rfc3339).ok())
            .expect("a near-future unix timestamp formats as RFC-3339")
    }
}

/// The deterministic quote id: `keccak256(token_in ‖ token_out ‖ amount_in)`, doubling as the
/// routing intent so the same request always yields the same id.
fn quote_hash(token_in: Address, token_out: Address, amount_in: U256) -> B256 {
    let mut bytes = Vec::with_capacity(20 + 20 + 32);
    bytes.extend_from_slice(token_in.as_slice());
    bytes.extend_from_slice(token_out.as_slice());
    bytes.extend_from_slice(&amount_in.to_be_bytes::<32>());
    keccak256(bytes)
}

/// Curve label per strategy on the pair, from the same snapshot the route was solved over.
fn curve_labels(
    snapshot: &Snapshot,
    token_in: Address,
    token_out: Address,
) -> BTreeMap<StrategyHash, &'static str> {
    snapshot
        .active_strategies_for_pair(TokenPair::new(token_in, token_out))
        .filter_map(|strategy| match &strategy.curve {
            CurveSpec::Priceable { curve, .. } => {
                Some((strategy.key.strategy_hash, curve_label(curve)))
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deps::ledger::{BudgetSource, BudgetSourceError, LedgerStore, LedgerStoreError};
    use crate::deps::routing::{GasPrice, GasPriceError, PriceOracle, PriceOracleError};
    use crate::primitives::asset::{TokenList, TokenMeta};
    use crate::primitives::ledger::{AccountKey, Reservation};
    use crate::primitives::registry::{Curve, CurveSpec, MakerStrategy, Snapshot, StrategyKey};
    use crate::primitives::{MakerId, ReservationId, UsdPrice};
    use rust_decimal::Decimal;

    const USDC: u8 = 1;
    const WETH: u8 = 2;

    fn addr(n: u8) -> Address {
        Address::from([n; 20])
    }

    fn e(n: u64, dec: u32) -> U256 {
        U256::from(n) * U256::from(10u64).pow(U256::from(dec))
    }

    fn meta(n: u8, symbol: &str, decimals: u8) -> TokenMeta {
        TokenMeta {
            chain_id: 31337,
            address: addr(n),
            symbol: symbol.to_string(),
            name: symbol.to_string(),
            decimals,
            logo_uri: None,
            tags: vec![],
        }
    }

    /// An active XYC over WETH(2)/USDC(1) with the given raw reserves, keyed to `maker`.
    fn xyc(maker: u8, weth: U256, usdc: U256) -> MakerStrategy {
        let key = StrategyKey {
            maker: MakerId(addr(maker)),
            app: Address::ZERO,
            strategy_hash: crate::primitives::StrategyHash(B256::from([maker; 32])),
        };
        let mut strategy = MakerStrategy::new(key, &[]);
        strategy.curve = CurveSpec::Priceable {
            curve: Curve::Xyc,
            fees_in_bps: vec![],
        };
        strategy.balances.insert(addr(WETH), weth);
        strategy.balances.insert(addr(USDC), usdc);
        strategy
    }

    /// Budget source: a strategy virtual is its registry balance, a wallet never binds.
    struct FakeBudget {
        registry: Arc<SharedSnapshot>,
    }
    #[async_trait::async_trait]
    impl BudgetSource for FakeBudget {
        async fn budget(&self, account: &AccountKey) -> Result<U256, BudgetSourceError> {
            Ok(match account {
                AccountKey::WalletBudget { .. } => U256::MAX,
                AccountKey::StrategyVirtual {
                    maker,
                    strategy_hash,
                    token,
                } => {
                    let key = StrategyKey {
                        maker: *maker,
                        app: Address::ZERO,
                        strategy_hash: *strategy_hash,
                    };
                    self.registry
                        .load()
                        .strategy(&key)
                        .map(|s| s.balance(token))
                        .unwrap_or(U256::ZERO)
                }
            })
        }
    }

    struct NoopStore;
    #[async_trait::async_trait]
    impl LedgerStore for NoopStore {
        async fn reserve(&self, _: &Reservation) -> Result<(), LedgerStoreError> {
            Ok(())
        }
        async fn post(&self, _: ReservationId, _: &[U256]) -> Result<(), LedgerStoreError> {
            Ok(())
        }
        async fn void(&self, _: ReservationId) -> Result<(), LedgerStoreError> {
            Ok(())
        }
        async fn expire(&self, _: ReservationId) -> Result<(), LedgerStoreError> {
            Ok(())
        }
        async fn void_reorg(&self, _: ReservationId) -> Result<(), LedgerStoreError> {
            Ok(())
        }
        async fn open_reservations(&self) -> Result<Vec<Reservation>, LedgerStoreError> {
            Ok(Vec::new())
        }
    }

    struct FixedClock;
    impl Clock for FixedClock {
        fn now_unix(&self) -> u64 {
            1_700_000_000
        }
    }

    /// A seedable market: `gas_wei == 0` and an empty price map read as "unavailable", so the quote
    /// routes gas-free (the common cold-cache path); seed both to exercise the gas cost.
    struct FakeMarket {
        gas_wei: u128,
        prices: BTreeMap<Address, UsdPrice>,
    }
    #[async_trait::async_trait]
    impl GasPrice for FakeMarket {
        async fn gas_price_wei(&self) -> Result<u128, GasPriceError> {
            match self.gas_wei {
                0 => Err(GasPriceError::Source("no gas".to_string())),
                wei => Ok(wei),
            }
        }
    }
    #[async_trait::async_trait]
    impl PriceOracle for FakeMarket {
        async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError> {
            self.prices
                .get(&token)
                .copied()
                .ok_or(PriceOracleError::NotFound(token))
        }
    }

    /// A quote service over `strategies` with the caps synced and the given market.
    async fn service_with(strategies: Vec<MakerStrategy>, market: Arc<FakeMarket>) -> QuoteService {
        let registry = Arc::new(SharedSnapshot::new(Snapshot::from_strategies(strategies)));
        let list = TokenList {
            name: "test".to_string(),
            tokens: vec![meta(USDC, "USDC", 6), meta(WETH, "WETH", 18)],
        };
        let assets = Arc::new(AssetManager::new(list, Arc::clone(&registry)));
        let ledger = Arc::new(LedgerService::new(
            Arc::new(NoopStore),
            Arc::new(FakeBudget {
                registry: Arc::clone(&registry),
            }),
            Arc::new(FixedClock),
        ));
        ledger.sync_budgets(&registry.load()).await.unwrap();
        let gas: Arc<dyn GasPrice> = market.clone();
        let oracle: Arc<dyn PriceOracle> = market.clone();
        let leg_cost = Arc::new(LegCostResolver::new(
            gas,
            oracle,
            Arc::clone(&assets),
            addr(WETH),
            150_000,
        ));
        let valuation = Arc::new(Valuation::new(market));
        QuoteService::new(
            registry,
            ledger,
            assets,
            RoutingConfig::new(16, 4, 150_000),
            Arc::new(FixedClock),
            leg_cost,
            valuation,
        )
    }

    /// Gas-free (cold market) service — the split is the pure marginal optimum.
    async fn service(strategies: Vec<MakerStrategy>) -> QuoteService {
        service_with(
            strategies,
            Arc::new(FakeMarket {
                gas_wei: 0,
                prices: BTreeMap::new(),
            }),
        )
        .await
    }

    #[tokio::test]
    async fn quotes_a_single_maker_route() {
        let svc = service(vec![xyc(3, e(100, 18), e(300_000, 6))]).await;
        let quote = svc
            .quote(addr(WETH), addr(USDC), e(1, 18))
            .await
            .expect("a route");

        assert_eq!(quote.makers_sourced, 1);
        assert_eq!(quote.legs.len(), 1);
        assert_eq!(quote.legs[0].curve, "XYC");
        assert!((quote.legs[0].share_pct - 100.0).abs() < 0.01);
        assert!(quote.legs[0].amount_out.raw.parse::<u128>().unwrap() > 0);
        assert!(quote.amount_out.raw.parse::<u128>().unwrap() > 0);
        assert!(quote.quote_id.starts_with("0x"));
        assert!(quote.expires_at.contains('T'), "RFC-3339 instant");
    }

    #[tokio::test]
    async fn quote_id_is_deterministic_for_the_same_request() {
        let svc = service(vec![xyc(3, e(100, 18), e(300_000, 6))]).await;
        let a = svc.quote(addr(WETH), addr(USDC), e(1, 18)).await.unwrap();
        let b = svc.quote(addr(WETH), addr(USDC), e(1, 18)).await.unwrap();
        assert_eq!(a.quote_id, b.quote_id);
    }

    #[tokio::test]
    async fn shares_sum_to_one_hundred_across_the_split() {
        let svc = service(vec![
            xyc(3, e(100, 18), e(300_000, 6)),
            xyc(4, e(100, 18), e(300_000, 6)),
        ])
        .await;
        let quote = svc
            .quote(addr(WETH), addr(USDC), e(50, 18))
            .await
            .expect("a route");
        let total: f64 = quote.legs.iter().map(|leg| leg.share_pct).sum();
        assert!(
            (total - 100.0).abs() < 0.5,
            "shares sum to ~100, got {total}"
        );
    }

    #[tokio::test]
    async fn no_makers_yields_no_route() {
        let svc = service(vec![]).await;
        assert!(svc.quote(addr(WETH), addr(USDC), e(1, 18)).await.is_none());
    }

    #[tokio::test]
    async fn gas_cost_prunes_the_split_to_one_leg() {
        // Two makers the gas-free optimum would split; a large per-leg gas (priced from the cache)
        // makes a second leg not worth it, so the same trade collapses to one.
        let strategies = vec![
            xyc(3, e(100, 18), e(300_000, 6)),
            xyc(4, e(100, 18), e(300_000, 6)),
        ];
        let free = service(strategies.clone()).await;
        let split = free.quote(addr(WETH), addr(USDC), e(50, 18)).await.unwrap();
        assert!(split.makers_sourced >= 2, "gas-free optimum fragments");

        let priced = Arc::new(FakeMarket {
            gas_wei: 1_000_000_000_000_000, // absurd gas ⇒ no second leg earns it
            prices: BTreeMap::from([
                (addr(WETH), UsdPrice(Decimal::from(3000u64))),
                (addr(USDC), UsdPrice(Decimal::from(1u64))),
            ]),
        });
        let svc = service_with(strategies, priced).await;
        let quote = svc
            .quote(addr(WETH), addr(USDC), e(50, 18))
            .await
            .expect("still routes");
        assert_eq!(quote.makers_sourced, 1, "gas collapses the split");
    }
}
