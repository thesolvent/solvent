//! The swap decision loop for one taker-signed intent: route it exact-out (deliver the order's
//! output, spend within its input), reserve the plan, persist the trade, and submit the fill. Runs
//! synchronously behind `POST /swap`; the reconcile worker later settles the trade. Protocol-agnostic
//! — it takes an already-normalized [`Intent`] (the adapter decodes/verifies/cosigns the order).

use std::sync::Arc;

use alloy_primitives::{keccak256, Address, U256};

use crate::asset::AssetManager;
use crate::deps::ingest::FillBuilder;
use crate::deps::ledger::Clock;
use crate::deps::routing::{GasPrice, PriceOracle};
use crate::deps::trade::{Settlement, TradeStore};
use crate::execution::ExecutionService;
use crate::ledger::LedgerService;
use crate::obs::warn;
use crate::primitives::execution::{FillOutcome, FillTx, PendingFill};
use crate::primitives::ingest::Intent;
use crate::primitives::ledger::{LedgerError, ReservationSource};
use crate::primitives::routing::{RoutePlan, RouteRequest, RoutingConfig};
use crate::primitives::trade::{Trade, TradeAttempt, TradeId, TradeLeg, TradeStatus};
use crate::primitives::{IntentId, ReservationId};
use crate::registry::SharedSnapshot;
use crate::routing::{resolve_leg_cost, route};
use crate::SolventError;

/// The chain/fill constants and routing knobs the swap path needs, bundled to keep the constructor
/// small.
pub struct SwapConfig {
    pub routing: RoutingConfig,
    /// Native token (WETH) for gas valuation.
    pub native: Address,
    pub chain_id: u64,
    /// The filler contract and the account authorized to call it.
    pub filler: Address,
    pub filler_owner: Address,
    /// How long a reservation holds before the TTL sweep may release it.
    pub reservation_ttl_secs: u64,
}

/// The result of submitting a swap: the trade's id and where its lifecycle stands.
pub struct SwapOutcome {
    pub trade_id: TradeId,
    pub status: TradeStatus,
}

pub struct SwapService {
    registry: Arc<SharedSnapshot>,
    ledger: Arc<LedgerService>,
    trades: Arc<dyn TradeStore>,
    execution: Arc<ExecutionService>,
    fill_builder: Arc<dyn FillBuilder>,
    assets: Arc<AssetManager>,
    gas: Arc<dyn GasPrice>,
    oracle: Arc<dyn PriceOracle>,
    clock: Arc<dyn Clock>,
    config: SwapConfig,
}

impl SwapService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        registry: Arc<SharedSnapshot>,
        ledger: Arc<LedgerService>,
        trades: Arc<dyn TradeStore>,
        execution: Arc<ExecutionService>,
        fill_builder: Arc<dyn FillBuilder>,
        assets: Arc<AssetManager>,
        gas: Arc<dyn GasPrice>,
        oracle: Arc<dyn PriceOracle>,
        clock: Arc<dyn Clock>,
        config: SwapConfig,
    ) -> Self {
        Self {
            registry,
            ledger,
            trades,
            execution,
            fill_builder,
            assets,
            gas,
            oracle,
            clock,
            config,
        }
    }

    /// Submit one taker-signed intent. Idempotent on the order hash: a resubmit returns the existing
    /// trade without re-reserving or re-filling. `taker` is the order's swapper (the adapter reads it
    /// from the decoded order); `trade_id` is a freshly-minted candidate the store keeps only if this
    /// order is new.
    pub async fn submit(
        &self,
        intent: Intent,
        taker: Address,
        trade_id: TradeId,
    ) -> Result<SwapOutcome, SolventError> {
        let now = self.clock.now_unix();
        let token_in = intent.input.token;
        let Some(output) = intent.outputs.first() else {
            return self.declined(trade_id, &intent, taker, now).await;
        };
        let token_out = output.token;
        let amount_in = intent.input.curve.amount_at(now);
        let min_out = output.curve.amount_at(now);

        let snapshot = self.registry.load();
        let caps = self.ledger.snapshot();
        let request = RouteRequest {
            intent: intent.id,
            token_in,
            token_out,
            amount: min_out,
            exact_in: false,
        };
        // Exact-out charges gas in the spread token = `token_in`. Cache-read, gas-free if unpriced.
        let per_leg_cost = resolve_leg_cost(
            self.gas.as_ref(),
            self.oracle.as_ref(),
            self.config.routing.gas_units_per_leg,
            self.config.native,
            token_in,
            self.decimals(token_in),
        )
        .await
        .unwrap_or(U256::ZERO);
        // The taker's input is the max-in bound: a plan that can't source the output within it (net
        // of gas) is unprofitable, so the router declines.
        let Some(plan) = route(
            &snapshot,
            &caps,
            &request,
            amount_in,
            &self.config.routing,
            per_leg_cost,
            None,
        ) else {
            return self.declined(trade_id, &intent, taker, now).await;
        };

        // Persist first (dedup on the order hash); only a newly-recorded order reserves and fills.
        let trade = self.trade(
            trade_id,
            &intent,
            taker,
            token_in,
            token_out,
            amount_in,
            min_out,
            Some(&plan),
            now,
            TradeStatus::Quoted,
        );
        let legs = trade_legs(&plan);
        let created = self
            .trades
            .create(
                &trade,
                &legs,
                &reached(now, &[TradeStatus::Created, TradeStatus::Quoted]),
            )
            .await?;
        if !created.created {
            let status = self
                .trades
                .info(&created.id)
                .await?
                .map_or(TradeStatus::Quoted, |info| info.trade.status);
            return Ok(SwapOutcome {
                trade_id: created.id,
                status,
            });
        }

        let reservation = reservation_id(intent.id, &plan);
        match self
            .ledger
            .reserve(
                reservation,
                intent.id,
                sources_of(&plan),
                self.config.reservation_ttl_secs,
            )
            .await
        {
            Ok(()) => {}
            // Capacity taken between quote and reserve is a decline, not a failure; anything else
            // (store/infra) propagates.
            Err(SolventError::Ledger(LedgerError::Insufficient(_))) => {
                warn!(intent = %intent.id, "reserve declined: insufficient capacity");
                self.settle(&created.id, TradeStatus::Declined, now).await?;
                return Ok(SwapOutcome {
                    trade_id: created.id,
                    status: TradeStatus::Declined,
                });
            }
            Err(e) => return Err(e),
        }
        self.trades
            .advance(&created.id, TradeStatus::Reserved, now)
            .await?;

        let calldata = self.fill_builder.build(&intent, &plan, &snapshot)?;
        let pending = PendingFill::new(
            FillTx::new(
                intent.id,
                self.config.chain_id,
                self.config.filler_owner,
                self.config.filler,
                calldata,
            ),
            reservation,
        );
        match self.execution.fill(pending).await? {
            FillOutcome::Submitted { .. } => {
                self.trades
                    .advance(&created.id, TradeStatus::Simulated, now)
                    .await?;
                self.trades
                    .advance(&created.id, TradeStatus::Submitted, now)
                    .await?;
                Ok(SwapOutcome {
                    trade_id: created.id,
                    status: TradeStatus::Submitted,
                })
            }
            // The sim gate already voided the reservation; just mark the trade declined.
            FillOutcome::Rejected { .. } => {
                self.settle(&created.id, TradeStatus::Declined, now).await?;
                Ok(SwapOutcome {
                    trade_id: created.id,
                    status: TradeStatus::Declined,
                })
            }
        }
    }

    /// Record an unroutable order as a declined trade (no legs), so it still shows in the explorer.
    async fn declined(
        &self,
        trade_id: TradeId,
        intent: &Intent,
        taker: Address,
        now: u64,
    ) -> Result<SwapOutcome, SolventError> {
        let token_out = intent.outputs.first().map_or(Address::ZERO, |o| o.token);
        let min_out = intent
            .outputs
            .first()
            .map_or(U256::ZERO, |o| o.curve.amount_at(now));
        let trade = self.trade(
            trade_id,
            intent,
            taker,
            intent.input.token,
            token_out,
            intent.input.curve.amount_at(now),
            min_out,
            None,
            now,
            TradeStatus::Declined,
        );
        let created = self
            .trades
            .create(
                &trade,
                &[],
                &reached(now, &[TradeStatus::Created, TradeStatus::Declined]),
            )
            .await?;
        Ok(SwapOutcome {
            trade_id: created.id,
            status: TradeStatus::Declined,
        })
    }

    async fn settle(&self, id: &TradeId, status: TradeStatus, at: u64) -> Result<(), SolventError> {
        self.trades
            .settle(
                id,
                &Settlement {
                    status,
                    amount_out: None,
                    tx_hash: None,
                    block_number: None,
                    at,
                },
            )
            .await
            .map_err(SolventError::from)
    }

    #[allow(clippy::too_many_arguments)]
    fn trade(
        &self,
        id: TradeId,
        intent: &Intent,
        taker: Address,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
        min_amount_out: U256,
        plan: Option<&RoutePlan>,
        now: u64,
        status: TradeStatus,
    ) -> Trade {
        Trade {
            id,
            order_hash: intent.id,
            taker,
            token_in,
            token_out,
            amount_in,
            min_amount_out,
            amount_out: None,
            status,
            deadline_block: intent.deadline,
            signature: Some(intent.signature.clone()),
            price_impact_pct: None,
            surplus: plan.map(|p| p.expected_profit),
            tx_hash: None,
            block_number: None,
            created_at: now,
            settled_at: None,
        }
    }

    fn decimals(&self, token: Address) -> u8 {
        self.assets.token(&token).map_or(18, |token| token.decimals)
    }
}

/// The reservation id: `keccak(intentId ‖ routePlanHash)`, so it is stable for one intent+plan and a
/// duplicate reserve is a no-op.
fn reservation_id(intent: IntentId, plan: &RoutePlan) -> ReservationId {
    let mut bytes = Vec::with_capacity(32 + plan.legs.len() * 104);
    bytes.extend_from_slice(intent.0.as_slice());
    for leg in &plan.legs {
        bytes.extend_from_slice(leg.maker.0.as_slice());
        bytes.extend_from_slice(leg.strategy_hash.0.as_slice());
        bytes.extend_from_slice(leg.token_out.as_slice());
        bytes.extend_from_slice(&leg.amount_out.to_be_bytes::<32>());
    }
    ReservationId(keccak256(bytes))
}

/// The reservation sources a plan holds: each leg reserves its output on the payout token.
fn sources_of(plan: &RoutePlan) -> Vec<ReservationSource> {
    plan.legs
        .iter()
        .map(|leg| ReservationSource {
            maker: leg.maker,
            strategy_hash: leg.strategy_hash,
            token: leg.token_out,
            amount: leg.amount_out,
        })
        .collect()
}

/// The persisted legs of a plan (leg-specific data only; tokens/tx come from the parent trade).
fn trade_legs(plan: &RoutePlan) -> Vec<TradeLeg> {
    plan.legs
        .iter()
        .map(|leg| TradeLeg {
            maker: leg.maker,
            strategy_hash: leg.strategy_hash,
            amount_in: leg.amount_in,
            amount_out: leg.amount_out,
        })
        .collect()
}

/// The lifecycle stages reached at persist time, each stamped `now`.
fn reached(now: u64, stages: &[TradeStatus]) -> Vec<TradeAttempt> {
    stages
        .iter()
        .map(|&status| TradeAttempt { status, at: now })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use alloy_primitives::{Bytes, B256};
    use async_trait::async_trait;

    use crate::deps::execution::{
        Execution, ExecutionError, SettlementError, SettlementReader, SimError, SimGate,
    };
    use crate::deps::ingest::FillBuilderError;
    use crate::deps::ledger::{BudgetSource, BudgetSourceError, LedgerStore, LedgerStoreError};
    use crate::deps::routing::{GasPriceError, PriceOracleError};
    use crate::deps::trade::{CreateResult, Page, TradeFilter, TradeStats, TradeStoreError};
    use crate::primitives::asset::{TokenList, TokenMeta};
    use crate::primitives::execution::{ExecHandle, ExecStatus, SimVerdict, TrackedFill};
    use crate::primitives::ingest::{AmountCurve, IntentInput, IntentOutput, ProtocolId};
    use crate::primitives::ledger::{AccountKey, Reservation};
    use crate::primitives::registry::{Curve, CurveSpec, MakerStrategy, Snapshot, StrategyKey};
    use crate::primitives::trade::TradeInfo;
    use crate::primitives::{MakerId, StrategyHash, UsdPrice};
    use ulid::Ulid;

    const USDC: u8 = 1;
    const WETH: u8 = 2;

    fn addr(n: u8) -> Address {
        Address::from([n; 20])
    }
    fn e(n: u64, dec: u32) -> U256 {
        U256::from(n) * U256::from(10u64).pow(U256::from(dec))
    }

    /// An exact-out intent: pay up to `amount_in` WETH, deliver `min_out` USDC to `taker`.
    fn intent(order: u8, taker: Address, amount_in: U256, min_out: U256) -> Intent {
        Intent::new(
            IntentId(B256::from([order; 32])),
            ProtocolId::UniswapXV2,
            IntentInput::new(addr(WETH), AmountCurve::scalar(amount_in)),
            vec![IntentOutput::new(
                addr(USDC),
                AmountCurve::scalar(min_out),
                taker,
            )],
            2_000_000_000,
            None,
            addr(0xEE),
            crate::primitives::ChainId(31337),
            Bytes::from(vec![0xab]),
            Bytes::from(vec![0xcd]),
            0,
        )
    }

    fn xyc(maker: u8, weth: U256, usdc: U256) -> MakerStrategy {
        let key = StrategyKey {
            maker: MakerId(addr(maker)),
            app: Address::ZERO,
            strategy_hash: StrategyHash(B256::from([maker; 32])),
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

    struct FakeBudget {
        registry: Arc<SharedSnapshot>,
    }
    #[async_trait]
    impl BudgetSource for FakeBudget {
        async fn budget(&self, account: &AccountKey) -> Result<U256, BudgetSourceError> {
            Ok(match account {
                AccountKey::WalletBudget { .. } => U256::MAX,
                AccountKey::StrategyVirtual {
                    maker,
                    strategy_hash,
                    token,
                } => self
                    .registry
                    .load()
                    .strategy(&StrategyKey {
                        maker: *maker,
                        app: Address::ZERO,
                        strategy_hash: *strategy_hash,
                    })
                    .map(|s| s.balance(token))
                    .unwrap_or(U256::ZERO),
            })
        }
    }

    struct NoopLedgerStore;
    #[async_trait]
    impl LedgerStore for NoopLedgerStore {
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

    struct NoMarket;
    #[async_trait]
    impl GasPrice for NoMarket {
        async fn gas_price_wei(&self) -> Result<u128, GasPriceError> {
            Err(GasPriceError::Source("none".into()))
        }
    }
    #[async_trait]
    impl PriceOracle for NoMarket {
        async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError> {
            Err(PriceOracleError::NotFound(token))
        }
    }

    struct FakeFill;
    impl FillBuilder for FakeFill {
        fn build(
            &self,
            _: &Intent,
            _: &RoutePlan,
            _: &Snapshot,
        ) -> Result<Bytes, FillBuilderError> {
            Ok(Bytes::from(vec![0x01, 0x02]))
        }
    }

    struct FakeSim(SimVerdict);
    #[async_trait]
    impl SimGate for FakeSim {
        async fn simulate(&self, _: &FillTx) -> Result<SimVerdict, SimError> {
            Ok(self.0.clone())
        }
    }
    struct FakeExec;
    #[async_trait]
    impl Execution for FakeExec {
        async fn submit(
            &self,
            fill: &FillTx,
            _: ReservationId,
        ) -> Result<ExecHandle, ExecutionError> {
            Ok(ExecHandle(fill.intent.0))
        }
        async fn status(&self, _: ExecHandle) -> Result<Option<ExecStatus>, ExecutionError> {
            Ok(Some(ExecStatus::Pending))
        }
        async fn forget(&self, _: IntentId) -> Result<(), ExecutionError> {
            Ok(())
        }
        async fn tracked(&self) -> Result<Vec<TrackedFill>, ExecutionError> {
            Ok(Vec::new())
        }
        async fn tick(&self) -> Result<(), ExecutionError> {
            Ok(())
        }
    }
    struct FakeSettle;
    #[async_trait]
    impl SettlementReader for FakeSettle {
        async fn settled(
            &self,
            _: B256,
            _: &[ReservationSource],
        ) -> Result<Vec<U256>, SettlementError> {
            Ok(Vec::new())
        }
    }

    /// A minimal in-memory trade store: enough to assert create/dedup/advance/settle.
    #[derive(Default)]
    struct MemTrades {
        rows: Mutex<BTreeMap<B256, TradeInfo>>, // keyed by order hash
    }
    #[async_trait]
    impl TradeStore for MemTrades {
        async fn create(
            &self,
            trade: &Trade,
            legs: &[TradeLeg],
            attempts: &[TradeAttempt],
        ) -> Result<CreateResult, TradeStoreError> {
            let mut rows = self.rows.lock().unwrap();
            if let Some(existing) = rows.get(&trade.order_hash.0) {
                return Ok(CreateResult {
                    id: existing.trade.id,
                    created: false,
                });
            }
            rows.insert(
                trade.order_hash.0,
                TradeInfo {
                    trade: trade.clone(),
                    attempts: attempts.to_vec(),
                    legs: legs.to_vec(),
                },
            );
            Ok(CreateResult {
                id: trade.id,
                created: true,
            })
        }
        async fn advance(
            &self,
            id: &TradeId,
            status: TradeStatus,
            at: u64,
        ) -> Result<(), TradeStoreError> {
            let mut rows = self.rows.lock().unwrap();
            if let Some(info) = rows.values_mut().find(|i| i.trade.id == *id) {
                if status.rank() > info.trade.status.rank() {
                    info.trade.status = status;
                }
                info.attempts.push(TradeAttempt { status, at });
            }
            Ok(())
        }
        async fn settle(&self, id: &TradeId, outcome: &Settlement) -> Result<(), TradeStoreError> {
            let mut rows = self.rows.lock().unwrap();
            if let Some(info) = rows.values_mut().find(|i| i.trade.id == *id) {
                if info.trade.settled_at.is_none() {
                    info.trade.status = outcome.status;
                    info.trade.amount_out = outcome.amount_out;
                    info.trade.settled_at = Some(outcome.at);
                }
            }
            Ok(())
        }
        async fn info(&self, id: &TradeId) -> Result<Option<TradeInfo>, TradeStoreError> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .values()
                .find(|i| i.trade.id == *id)
                .cloned())
        }
        async fn find_by_order(
            &self,
            order_hash: &IntentId,
        ) -> Result<Option<Trade>, TradeStoreError> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .get(&order_hash.0)
                .map(|i| i.trade.clone()))
        }
        async fn list(&self, _: &TradeFilter, _: &Page) -> Result<Vec<Trade>, TradeStoreError> {
            Ok(Vec::new())
        }
        async fn stats(&self) -> Result<TradeStats, TradeStoreError> {
            Ok(TradeStats {
                settled: 0,
                confirmed: 0,
                median_impact_pct: None,
            })
        }
    }

    struct Harness {
        swap: SwapService,
        trades: Arc<MemTrades>,
        ledger: Arc<LedgerService>,
    }

    async fn harness(strategies: Vec<MakerStrategy>, sim: SimVerdict) -> Harness {
        let registry = Arc::new(SharedSnapshot::new(Snapshot::from_strategies(strategies)));
        let list = TokenList {
            name: "t".into(),
            tokens: vec![
                TokenMeta {
                    chain_id: 31337,
                    address: addr(USDC),
                    symbol: "USDC".into(),
                    name: "USDC".into(),
                    decimals: 6,
                    logo_uri: None,
                    tags: vec![],
                },
                TokenMeta {
                    chain_id: 31337,
                    address: addr(WETH),
                    symbol: "WETH".into(),
                    name: "WETH".into(),
                    decimals: 18,
                    logo_uri: None,
                    tags: vec![],
                },
            ],
        };
        let assets = Arc::new(AssetManager::new(list, Arc::clone(&registry)));
        let ledger = Arc::new(LedgerService::new(
            Arc::new(NoopLedgerStore),
            Arc::new(FakeBudget {
                registry: Arc::clone(&registry),
            }),
            Arc::new(FixedClock),
        ));
        ledger.sync_budgets(&registry.load()).await.unwrap();
        let execution = Arc::new(ExecutionService::new(
            Arc::new(FakeSim(sim)),
            Arc::new(FakeExec),
            Arc::new(FakeSettle),
            Arc::clone(&ledger),
        ));
        let trades = Arc::new(MemTrades::default());
        let swap = SwapService::new(
            Arc::clone(&registry),
            Arc::clone(&ledger),
            trades.clone(),
            execution,
            Arc::new(FakeFill),
            assets,
            Arc::new(NoMarket),
            Arc::new(NoMarket),
            Arc::new(FixedClock),
            SwapConfig {
                routing: RoutingConfig::new(16, 4, 150_000),
                native: addr(WETH),
                chain_id: 31337,
                filler: addr(0xF1),
                filler_owner: addr(0xF0),
                reservation_ttl_secs: 60,
            },
        );
        Harness {
            swap,
            trades,
            ledger,
        }
    }

    fn tid() -> TradeId {
        TradeId(Ulid::from_parts(1_700_000_000_000, 1))
    }

    fn virt(maker: u8, token: u8) -> AccountKey {
        AccountKey::StrategyVirtual {
            maker: MakerId(addr(maker)),
            strategy_hash: StrategyHash(B256::from([maker; 32])),
            token: addr(token),
        }
    }

    #[tokio::test]
    async fn routable_order_reserves_and_submits() {
        let h = harness(vec![xyc(3, e(100, 18), e(300_000, 6))], SimVerdict::Ok).await;
        let before = h.ledger.available(&virt(3, USDC));
        let out = h
            .swap
            .submit(intent(1, addr(9), e(2, 18), e(3000, 6)), addr(9), tid())
            .await
            .unwrap();

        assert_eq!(out.status, TradeStatus::Submitted);
        let info = h.trades.info(&out.trade_id).await.unwrap().unwrap();
        assert_eq!(info.legs.len(), 1);
        assert_eq!(info.trade.taker, addr(9));
        // The payout was held.
        assert!(h.ledger.available(&virt(3, USDC)) < before);
    }

    #[tokio::test]
    async fn unroutable_order_is_declined() {
        // No makers ⇒ no route.
        let h = harness(vec![], SimVerdict::Ok).await;
        let out = h
            .swap
            .submit(intent(1, addr(9), e(2, 18), e(3000, 6)), addr(9), tid())
            .await
            .unwrap();
        assert_eq!(out.status, TradeStatus::Declined);
        assert!(h
            .trades
            .info(&out.trade_id)
            .await
            .unwrap()
            .unwrap()
            .legs
            .is_empty());
    }

    #[tokio::test]
    async fn resubmit_is_idempotent() {
        let h = harness(vec![xyc(3, e(100, 18), e(300_000, 6))], SimVerdict::Ok).await;
        let order = intent(7, addr(9), e(2, 18), e(3000, 6));
        let first = h.swap.submit(order.clone(), addr(9), tid()).await.unwrap();
        let held = h.ledger.available(&virt(3, USDC));
        // A second submit of the same order (new candidate id) neither re-reserves nor re-fills.
        let again = h
            .swap
            .submit(
                order,
                addr(9),
                TradeId(Ulid::from_parts(1_700_000_000_999, 2)),
            )
            .await
            .unwrap();
        assert_eq!(again.trade_id, first.trade_id);
        assert_eq!(h.ledger.available(&virt(3, USDC)), held, "no second hold");
    }

    #[tokio::test]
    async fn sim_reject_declines_and_releases_the_hold() {
        let h = harness(
            vec![xyc(3, e(100, 18), e(300_000, 6))],
            SimVerdict::Reject {
                reason: "stale".into(),
            },
        )
        .await;
        let before = h.ledger.available(&virt(3, USDC));
        let out = h
            .swap
            .submit(intent(1, addr(9), e(2, 18), e(3000, 6)), addr(9), tid())
            .await
            .unwrap();
        assert_eq!(out.status, TradeStatus::Declined);
        assert_eq!(
            h.ledger.available(&virt(3, USDC)),
            before,
            "sim reject voided the reservation"
        );
    }
}
