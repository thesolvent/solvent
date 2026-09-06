//! The recapture service: on each confirmed fill, value its legs against the oracle mid, split the
//! recaptured LVR back to the makers it rebalanced (via
//! [`recapture_split`](crate::primitives::recapture::recapture_split)), and accrue the credits.
//! Fail-closed on the credit — a missing price or a store error is logged, never surfaced to fail
//! the fill.

use std::collections::BTreeMap;
use std::sync::Arc;

use alloy_primitives::{Address, B256, U256};

use crate::deps::recapture::{RecaptureStore, SettledLegsReader};
use crate::deps::routing::PriceOracle;
use crate::obs::warn;
use crate::primitives::recapture::{recapture_split, RecaptureCredit, RecapturePolicy, TokenValue};
use crate::primitives::routing::RouteLeg;
use crate::primitives::IntentId;

/// Values a confirmed fill's legs and accrues the maker rebates it earned.
pub struct RecaptureService {
    oracle: Arc<dyn PriceOracle>,
    store: Arc<dyn RecaptureStore>,
    settled_legs: Arc<dyn SettledLegsReader>,
    /// Per-token base-unit decimals, from the composition root's token metadata.
    decimals: BTreeMap<Address, u8>,
    policy: RecapturePolicy,
}

impl RecaptureService {
    pub fn new(
        oracle: Arc<dyn PriceOracle>,
        store: Arc<dyn RecaptureStore>,
        settled_legs: Arc<dyn SettledLegsReader>,
        decimals: BTreeMap<Address, u8>,
        policy: RecapturePolicy,
    ) -> Self {
        Self {
            oracle,
            store,
            settled_legs,
            decimals,
            policy,
        }
    }

    /// Compute and accrue the recapture credits for a confirmed fill of `intent` settled by `tx`. The
    /// `plan_legs` supply each leg's identity (maker, strategy, token sides); the *actual* amounts are
    /// read from the settlement, so a partial fill credits only what really moved. `fill_spread` (in
    /// `spread_token`) bounds the total rebate. Best-effort/fail-closed: an unreadable settlement, an
    /// unpriceable token, or a store error drops the credit and logs — none is propagated, so
    /// recapture can never fail a settled fill. Returns the credits computed (empty when nothing was
    /// recaptured).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(intent = %intent)))]
    pub async fn on_settled(
        &self,
        intent: IntentId,
        tx: B256,
        plan_legs: &[RouteLeg],
        fill_spread: U256,
        spread_token: Address,
    ) -> Vec<RecaptureCredit> {
        let Ok(legs) = self.settled_legs.actual_legs(tx, plan_legs).await else {
            warn!(intent = %intent, "settled legs unreadable; nothing recaptured");
            return Vec::new();
        };
        let prices = self.prices_for(&legs, spread_token).await;
        let credits = recapture_split(&legs, &prices, fill_spread, spread_token, &self.policy);
        if !credits.is_empty() && self.store.accrue(intent, &credits).await.is_err() {
            warn!(intent = %intent, "recapture credits computed but not accrued");
        }
        credits
    }

    /// Price every token the fill touched, skipping any the oracle cannot price or whose decimals are
    /// unknown — those legs then recapture nothing.
    async fn prices_for(
        &self,
        legs: &[RouteLeg],
        spread_token: Address,
    ) -> BTreeMap<Address, TokenValue> {
        let mut tokens: Vec<Address> = legs
            .iter()
            .flat_map(|leg| [leg.token_in, leg.token_out])
            .chain(std::iter::once(spread_token))
            .collect();
        tokens.sort_unstable();
        tokens.dedup();

        let mut prices = BTreeMap::new();
        for token in tokens {
            let Some(&decimals) = self.decimals.get(&token) else {
                continue;
            };
            let Ok(price) = self.oracle.price(token).await else {
                warn!(token = %token, "no oracle price; leg not recaptured");
                continue;
            };
            prices.insert(token, TokenValue { price, decimals });
        }
        prices
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use alloy_primitives::B256;
    use async_trait::async_trait;
    use rust_decimal::Decimal;

    use crate::deps::recapture::{RecaptureStoreError, SettledLegsError};
    use crate::deps::routing::PriceOracleError;
    use crate::ledger::AvailableSnapshot;
    use crate::primitives::ledger::AccountKey;
    use crate::primitives::recapture::AccruedCredit;
    use crate::primitives::registry::{Curve, CurveSpec, MakerStrategy, Snapshot, StrategyKey};
    use crate::primitives::routing::{RouteRequest, RoutingConfig};
    use crate::primitives::{Bps, MakerId, StrategyHash, Usd, UsdPrice};
    use crate::routing::route;

    fn tok(n: u8) -> Address {
        Address::from([n; 20])
    }
    fn eth() -> Address {
        tok(1)
    }
    fn usdc() -> Address {
        tok(2)
    }
    fn maker(n: u8) -> MakerId {
        MakerId(Address::from([n; 20]))
    }
    fn e18(n: u64) -> U256 {
        U256::from(n) * U256::from(10u64).pow(U256::from(18u64))
    }
    fn e6(n: u64) -> U256 {
        U256::from(n) * U256::from(10u64).pow(U256::from(6u64))
    }

    struct FakeOracle(HashMap<Address, UsdPrice>);
    #[async_trait]
    impl PriceOracle for FakeOracle {
        async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError> {
            self.0
                .get(&token)
                .copied()
                .ok_or(PriceOracleError::NotFound(token))
        }
    }

    #[derive(Default)]
    struct MemStore(Mutex<Vec<AccruedCredit>>);
    #[async_trait]
    impl RecaptureStore for MemStore {
        async fn accrue(
            &self,
            intent: IntentId,
            credits: &[RecaptureCredit],
        ) -> Result<(), RecaptureStoreError> {
            let mut rows = self.0.lock().expect("lock");
            for credit in credits {
                rows.push(AccruedCredit::new(intent, credit.clone()));
            }
            Ok(())
        }
        async fn outstanding(&self) -> Result<Vec<AccruedCredit>, RecaptureStoreError> {
            Ok(self.0.lock().expect("lock").clone())
        }
        async fn mark_settled(&self, credits: &[AccruedCredit]) -> Result<(), RecaptureStoreError> {
            self.0
                .lock()
                .expect("lock")
                .retain(|a| !credits.contains(a));
            Ok(())
        }
    }
    impl MemStore {
        fn credits(&self) -> Vec<RecaptureCredit> {
            self.0
                .lock()
                .expect("lock")
                .iter()
                .map(|a| a.credit.clone())
                .collect()
        }
    }

    /// Echoes the plan legs back as the actual legs — the full-fill case (actual == expected).
    struct EchoLegs;
    #[async_trait]
    impl SettledLegsReader for EchoLegs {
        async fn actual_legs(
            &self,
            _tx: B256,
            legs: &[RouteLeg],
        ) -> Result<Vec<RouteLeg>, SettledLegsError> {
            Ok(legs.to_vec())
        }
    }

    /// Returns fixed actual legs regardless of the plan — models a fill that moved other amounts.
    struct FixedLegs(Vec<RouteLeg>);
    #[async_trait]
    impl SettledLegsReader for FixedLegs {
        async fn actual_legs(
            &self,
            _tx: B256,
            _legs: &[RouteLeg],
        ) -> Result<Vec<RouteLeg>, SettledLegsError> {
            Ok(self.0.clone())
        }
    }

    /// Fails to read the settlement — the fail-closed case.
    struct ErrLegs;
    #[async_trait]
    impl SettledLegsReader for ErrLegs {
        async fn actual_legs(
            &self,
            _tx: B256,
            _legs: &[RouteLeg],
        ) -> Result<Vec<RouteLeg>, SettledLegsError> {
            Err(SettledLegsError::Read("boom".into()))
        }
    }

    /// ETH at $3000, USDC at $1.
    fn oracle() -> Arc<FakeOracle> {
        Arc::new(FakeOracle(HashMap::from([
            (eth(), UsdPrice(Decimal::from(3000u32))),
            (usdc(), UsdPrice(Decimal::from(1u32))),
        ])))
    }
    fn decimals() -> BTreeMap<Address, u8> {
        BTreeMap::from([(eth(), 18u8), (usdc(), 6u8)])
    }
    fn policy(maker_share: u32, spread_cap: u32, min_credit: u32) -> RecapturePolicy {
        RecapturePolicy {
            maker_share: Bps(Decimal::from(maker_share)),
            spread_cap: Bps(Decimal::from(spread_cap)),
            min_credit: Usd(Decimal::from(min_credit)),
        }
    }

    /// A reverse (`USDC→ETH`) leg: the maker receives `amount_in` USDC and gives `amount_out` ETH.
    fn reverse_leg(maker_byte: u8, amount_in: U256, amount_out: U256) -> RouteLeg {
        RouteLeg {
            maker: maker(maker_byte),
            strategy_hash: StrategyHash(B256::from([maker_byte; 32])),
            token_in: usdc(),
            token_out: eth(),
            amount_in,
            amount_out,
        }
    }

    // A confirmed fill's reverse leg is valued and its rebate accrued (design's worked numbers).
    #[tokio::test]
    async fn on_settled_accrues_the_credit() {
        let store = Arc::new(MemStore::default());
        let svc = RecaptureService::new(
            oracle(),
            store.clone(),
            Arc::new(EchoLegs),
            decimals(),
            policy(8000, 10000, 0),
        );
        let legs = [reverse_leg(5, e6(2700), e18(1))];
        let credits = svc
            .on_settled(IntentId(B256::ZERO), B256::ZERO, &legs, e6(300), usdc())
            .await;
        let expected = vec![RecaptureCredit {
            maker: maker(5),
            token: usdc(),
            amount: e6(240),
        }];
        assert_eq!(credits, expected);
        assert_eq!(store.credits(), expected, "the credit was accrued");
    }

    // Fail-closed: with no ETH price the leg can't be valued, so nothing is computed or accrued.
    #[tokio::test]
    async fn missing_oracle_price_accrues_nothing() {
        let store = Arc::new(MemStore::default());
        let sparse = Arc::new(FakeOracle(HashMap::from([(
            usdc(),
            UsdPrice(Decimal::from(1u32)),
        )])));
        let svc = RecaptureService::new(
            sparse,
            store.clone(),
            Arc::new(EchoLegs),
            decimals(),
            policy(8000, 10000, 0),
        );
        let legs = [reverse_leg(5, e6(2700), e18(1))];
        let credits = svc
            .on_settled(IntentId(B256::ZERO), B256::ZERO, &legs, e6(300), usdc())
            .await;
        assert!(credits.is_empty());
        assert!(store.credits().is_empty());
    }

    // Actuals, not the plan, drive the credit: a fill that moved only half of the planned leg
    // recaptures half the LVR (240 → 120 USDC), though the plan legs are unchanged.
    #[tokio::test]
    async fn actual_legs_scale_the_credit() {
        let store = Arc::new(MemStore::default());
        // The fill actually moved half: the maker received 1350 USDC and gave 0.5 ETH.
        let actual = vec![reverse_leg(5, e6(1350), e18(1) / U256::from(2u64))];
        let svc = RecaptureService::new(
            oracle(),
            store.clone(),
            Arc::new(FixedLegs(actual)),
            decimals(),
            policy(8000, 10000, 0),
        );
        let plan = [reverse_leg(5, e6(2700), e18(1))];
        let credits = svc
            .on_settled(IntentId(B256::ZERO), B256::ZERO, &plan, e6(300), usdc())
            .await;
        assert_eq!(credits.first().map(|c| c.amount), Some(e6(120)));
    }

    // Fail-closed: an unreadable settlement computes and accrues nothing (never fails the fill).
    #[tokio::test]
    async fn unreadable_settlement_accrues_nothing() {
        let store = Arc::new(MemStore::default());
        let svc = RecaptureService::new(
            oracle(),
            store.clone(),
            Arc::new(ErrLegs),
            decimals(),
            policy(8000, 10000, 0),
        );
        let plan = [reverse_leg(5, e6(2700), e18(1))];
        let credits = svc
            .on_settled(IntentId(B256::ZERO), B256::ZERO, &plan, e6(300), usdc())
            .await;
        assert!(credits.is_empty());
        assert!(store.credits().is_empty());
    }

    fn xyc_maker(maker_byte: u8, usdc_bal: U256, eth_bal: U256) -> MakerStrategy {
        let mut balances = BTreeMap::new();
        balances.insert(usdc(), usdc_bal);
        balances.insert(eth(), eth_bal);
        MakerStrategy {
            key: StrategyKey {
                maker: maker(maker_byte),
                app: Address::ZERO,
                strategy_hash: StrategyHash(B256::from([maker_byte; 32])),
            },
            curve: CurveSpec::Priceable {
                curve: Curve::Xyc,
                fees_in_bps: vec![],
            },
            balances,
            active: true,
            program: alloy_primitives::Bytes::new(),
        }
    }
    fn caps_on_eth(makers: &[u8], budget: U256) -> AvailableSnapshot {
        let mut m = BTreeMap::new();
        for &b in makers {
            m.insert(
                AccountKey::WalletBudget {
                    maker: maker(b),
                    token: eth(),
                },
                budget,
            );
            m.insert(
                AccountKey::StrategyVirtual {
                    maker: maker(b),
                    strategy_hash: StrategyHash(B256::from([b; 32])),
                    token: eth(),
                },
                budget,
            );
        }
        AvailableSnapshot(m)
    }

    // The walking skeleton: after forward flow leaves maker 5 ETH-cheap, a reverse `USDC→ETH` buy
    // routes into maker 5 (internalization is the router's natural behavior), and recapture credits
    // that same maker — end to end through the real router.
    #[tokio::test]
    async fn reverse_flow_routes_to_the_imbalanced_maker_and_recaptures() {
        // Maker 5 is ETH-cheap (imbalanced by prior forward flow); maker 6 is balanced at mid.
        let snap = Snapshot::from_strategies([
            xyc_maker(5, e6(250_000), e18(100)),
            xyc_maker(6, e6(300_000), e18(100)),
        ]);
        let caps = caps_on_eth(&[5, 6], e18(100));
        let req = RouteRequest {
            intent: IntentId(B256::ZERO),
            token_in: usdc(),
            token_out: eth(),
            amount: e6(3000),
            exact_in: true,
        };
        let cfg = RoutingConfig::new(64, 8, 150_000);
        let plan =
            route(&snap, &caps, &req, U256::from(1u64), &cfg, U256::ZERO, None).expect("a plan");
        assert!(
            plan.legs.iter().all(|l| l.maker == maker(5)),
            "the reverse buy internalizes into the imbalanced maker"
        );

        let store = Arc::new(MemStore::default());
        let svc = RecaptureService::new(
            oracle(),
            store.clone(),
            Arc::new(EchoLegs),
            decimals(),
            policy(8000, 10000, 0),
        );
        let credits = svc
            .on_settled(
                req.intent,
                B256::ZERO,
                &plan.legs,
                plan.expected_profit,
                eth(),
            )
            .await;
        assert!(!credits.is_empty(), "the rebalanced maker earned a credit");
        assert!(
            credits
                .iter()
                .all(|c| c.maker == maker(5) && c.token == usdc() && c.amount > U256::ZERO),
            "recapture credits the imbalanced maker, in the drained token"
        );
    }
}
