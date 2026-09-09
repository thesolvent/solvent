//! The swap decision loop for one taker-signed intent: route it exact-out (deliver the order's
//! output, spend within its input), reserve the plan, persist the trade, and submit the fill. Runs
//! synchronously behind `POST /swap`; the reconcile worker later settles the trade. Protocol-agnostic
//! — it takes an already-normalized [`Intent`] (the adapter decodes/verifies/cosigns the order).

use std::sync::Arc;

use alloy_primitives::{keccak256, Address, U256};

use crate::deps::ingest::FillBuilder;
use crate::deps::ledger::Clock;
use crate::deps::trade::{Settlement, TradeStore};
use crate::execution::ExecutionService;
use crate::ledger::LedgerService;
use crate::obs::warn;
use crate::primitives::execution::{FillOutcome, FillTx, PendingFill};
use crate::primitives::ingest::Intent;
use crate::primitives::ledger::{LedgerError, ReservationSource};
use crate::primitives::registry::Snapshot;
use crate::primitives::routing::{RoutePlan, RouteRequest, RoutingConfig};
use crate::primitives::trade::{Trade, TradeAttempt, TradeId, TradeLeg, TradeStatus};
use crate::primitives::{IntentId, ReservationId};
use crate::registry::SharedSnapshot;
use crate::routing::{route, LegCostResolver};
use crate::SolventError;

/// The chain/fill constants and routing knobs the swap path needs, bundled to keep the constructor
/// small.
pub struct SwapConfig {
    pub routing: RoutingConfig,
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
    leg_cost: Arc<LegCostResolver>,
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
        leg_cost: Arc<LegCostResolver>,
        clock: Arc<dyn Clock>,
        config: SwapConfig,
    ) -> Self {
        Self {
            registry,
            ledger,
            trades,
            execution,
            fill_builder,
            leg_cost,
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
        prices: TradePrices,
    ) -> Result<SwapOutcome, SolventError> {
        let now = self.clock.now_unix();
        let Some(amounts) = swap_amounts(&intent, self.config.filler, now) else {
            return self.declined(trade_id, &intent, taker, now, prices).await;
        };

        let snapshot = self.registry.load();
        let caps = self.ledger.snapshot();
        let request = RouteRequest {
            intent: intent.id,
            token_in: amounts.token_in,
            token_out: amounts.token_out,
            amount: amounts.min_out,
            exact_in: false,
        };
        // Exact-out charges gas in the spread token = `token_in`. Cache-read, gas-free if unpriced.
        let per_leg_cost = self.leg_cost.for_request(&request).await;
        // The taker's input is the max-in bound: a plan that can't source the output within it (net
        // of gas) is unprofitable, so the router declines.
        let Some(plan) = route(
            &snapshot,
            &caps,
            &request,
            amounts.amount_in,
            &self.config.routing,
            per_leg_cost,
            None,
        ) else {
            return self.declined(trade_id, &intent, taker, now, prices).await;
        };

        // Persist first (dedup on the order hash); only a newly-recorded order reserves and fills.
        let trade = self.trade(
            trade_id,
            &intent,
            taker,
            &amounts,
            Some(&plan),
            now,
            TradeStatus::Quoted,
            &prices,
        );
        let created = self
            .trades
            .create(
                &trade,
                &trade_legs(&plan),
                &reached(now, &[TradeStatus::Created, TradeStatus::Quoted]),
            )
            .await?;
        // A resubmit already past Quoted is in flight (or terminal): echo it untouched. One still at
        // Created/Quoted crashed before reserving, so fall through and re-drive it.
        if !created.created {
            let status = self
                .trades
                .info(&created.id)
                .await?
                .map_or(TradeStatus::Quoted, |info| info.trade.status);
            if !matches!(status, TradeStatus::Created | TradeStatus::Quoted) {
                return Ok(SwapOutcome {
                    trade_id: created.id,
                    status,
                });
            }
        }

        let reservation = reservation_id(intent.id, &plan);
        if let Some(declined) = self
            .reserve_or_decline(&created.id, intent.id, reservation, &plan, now)
            .await?
        {
            return Ok(declined);
        }
        self.trades
            .advance(&created.id, TradeStatus::Reserved, now)
            .await?;

        self.submit_fill(&created.id, &intent, &plan, &snapshot, reservation, now)
            .await
    }

    /// Reserve the plan's payouts. `Ok(None)` continues to the fill; `Ok(Some(..))` is a decline
    /// (capacity taken since the quote — settled, not an error); infra errors propagate.
    async fn reserve_or_decline(
        &self,
        id: &TradeId,
        intent_id: IntentId,
        reservation: ReservationId,
        plan: &RoutePlan,
        now: u64,
    ) -> Result<Option<SwapOutcome>, SolventError> {
        match self
            .ledger
            .reserve(
                reservation,
                intent_id,
                sources_of(plan),
                self.config.reservation_ttl_secs,
            )
            .await
        {
            Ok(()) => Ok(None),
            Err(SolventError::Ledger(LedgerError::Insufficient(_))) => {
                warn!(intent = %intent_id, "reserve declined: insufficient capacity");
                self.settle(id, TradeStatus::Declined, now).await?;
                Ok(Some(SwapOutcome {
                    trade_id: *id,
                    status: TradeStatus::Declined,
                }))
            }
            Err(e) => Err(e),
        }
    }

    /// Build and submit the fill: `Submitted` on success, or settle `Declined` when the sim gate
    /// rejects (it has already voided the reservation).
    async fn submit_fill(
        &self,
        id: &TradeId,
        intent: &Intent,
        plan: &RoutePlan,
        snapshot: &Snapshot,
        reservation: ReservationId,
        now: u64,
    ) -> Result<SwapOutcome, SolventError> {
        let calldata = self.fill_builder.build(intent, plan, snapshot)?;
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
                self.trades.advance(id, TradeStatus::Simulated, now).await?;
                self.trades.advance(id, TradeStatus::Submitted, now).await?;
                Ok(SwapOutcome {
                    trade_id: *id,
                    status: TradeStatus::Submitted,
                })
            }
            FillOutcome::Rejected { .. } => {
                self.settle(id, TradeStatus::Declined, now).await?;
                Ok(SwapOutcome {
                    trade_id: *id,
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
        prices: TradePrices,
    ) -> Result<SwapOutcome, SolventError> {
        let delivery = intent.required_output(self.config.filler, now);
        let amounts = SwapAmounts {
            token_in: intent.input.token,
            token_out: delivery.map_or(Address::ZERO, |d| d.token),
            amount_in: intent.input.curve.amount_at(now),
            min_out: delivery.map_or(U256::ZERO, |d| d.amount),
        };
        let trade = self.trade(
            trade_id,
            intent,
            taker,
            &amounts,
            None,
            now,
            TradeStatus::Declined,
            &prices,
        );
        let created = self
            .trades
            .create(
                &trade,
                &[],
                &reached(now, &[TradeStatus::Created, TradeStatus::Declined]),
            )
            .await?;
        // Stamp `settled_at` like every other decline path, so /stats counts them consistently.
        self.settle(&created.id, TradeStatus::Declined, now).await?;
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
        amounts: &SwapAmounts,
        plan: Option<&RoutePlan>,
        now: u64,
        status: TradeStatus,
        prices: &TradePrices,
    ) -> Trade {
        Trade {
            id,
            order_hash: intent.id,
            taker,
            token_in: amounts.token_in,
            token_out: amounts.token_out,
            amount_in: amounts.amount_in,
            min_amount_out: amounts.min_out,
            amount_out: None,
            status,
            deadline_block: intent.deadline,
            signature: Some(intent.signature.clone()),
            price_impact_pct: plan.map(|p| p.price_impact_pct),
            surplus: plan.map(|p| p.expected_profit),
            tx_hash: None,
            block_number: None,
            created_at: now,
            settled_at: None,
            token_in_price_usd: prices.token_in_usd,
            token_out_price_usd: prices.token_out_usd,
        }
    }
}

/// The USD prices of a swap's tokens at submit, captured by the adapter (which holds the oracle) and
/// persisted on the trade so fee and value figures stay at trade-time economics.
#[derive(Debug, Clone, Copy, Default)]
pub struct TradePrices {
    pub token_in_usd: Option<f64>,
    pub token_out_usd: Option<f64>,
}

/// The swap's tokens and exact-out bounds at quote time.
struct SwapAmounts {
    token_in: Address,
    token_out: Address,
    /// The taker's max input (the exact-out spend ceiling).
    amount_in: U256,
    /// The order's minimum delivered output.
    min_out: U256,
}

/// The swap's tokens and exact-out bounds, or `None` when the order has nothing this filler can
/// deliver. `min_out` is what the settler will actually collect: every output leg summed, and raised
/// by the exclusivity toll when the window belongs to another filler.
fn swap_amounts(intent: &Intent, filler: Address, now: u64) -> Option<SwapAmounts> {
    let delivery = intent.required_output(filler, now)?;
    Some(SwapAmounts {
        token_in: intent.input.token,
        token_out: delivery.token,
        amount_in: intent.input.curve.amount_at(now),
        min_out: delivery.amount,
    })
}

/// Bytes per plan leg in the reservation-id preimage: maker · strategy_hash · token · amount.
const RESERVATION_LEG_BYTES: usize = 20 + 32 + 20 + 32;

/// The reservation id: `keccak(intentId ‖ routePlanHash)`, so it is stable for one intent+plan and a
/// duplicate reserve is a no-op.
fn reservation_id(intent: IntentId, plan: &RoutePlan) -> ReservationId {
    // Preimage = the 32-byte intent id followed by each leg's fields.
    let mut bytes = Vec::with_capacity(32 + plan.legs.len() * RESERVATION_LEG_BYTES);
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

    use crate::asset::AssetManager;
    use crate::deps::execution::{
        Execution, ExecutionError, SettlementError, SettlementReader, SimError, SimGate,
    };
    use crate::deps::ingest::FillBuilderError;
    use crate::deps::ledger::{BudgetSource, BudgetSourceError, LedgerStore, LedgerStoreError};
    use crate::deps::routing::{GasPrice, GasPriceError, PriceOracle, PriceOracleError};
    use crate::deps::trade::{
        CreateResult, MakerFill, Page, TradeFilter, TradeStats, TradeStoreError,
    };
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
        Intent::new(crate::primitives::ingest::IntentParts {
            deadline: 2_000_000_000,
            settler: addr(0xEE),
            raw: Bytes::from(vec![0xab]),
            signature: Bytes::from(vec![0xcd]),
            ..crate::primitives::ingest::IntentParts::new(
                IntentId(B256::from([order; 32])),
                ProtocolId::UniswapXV2,
                taker,
                IntentInput::new(addr(WETH), AmountCurve::scalar(amount_in)),
                vec![IntentOutput::new(
                    addr(USDC),
                    AmountCurve::scalar(min_out),
                    taker,
                )],
                crate::primitives::ChainId(31337),
            )
        })
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
        async fn list_for_maker(
            &self,
            _: Address,
            _: &Page,
            _: Option<std::ops::Range<u64>>,
        ) -> Result<Vec<MakerFill>, TradeStoreError> {
            Ok(Vec::new())
        }
        async fn stats(&self) -> Result<TradeStats, TradeStoreError> {
            Ok(TradeStats {
                settled: 0,
                confirmed: 0,
                failed: 0,
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
        let leg_cost = Arc::new(LegCostResolver::new(
            Arc::new(NoMarket),
            Arc::new(NoMarket),
            assets,
            addr(WETH),
            150_000,
        ));
        let swap = SwapService::new(
            Arc::clone(&registry),
            Arc::clone(&ledger),
            trades.clone(),
            execution,
            Arc::new(FakeFill),
            leg_cost,
            Arc::new(FixedClock),
            SwapConfig {
                routing: RoutingConfig::new(16, 4, 150_000),
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
            .submit(
                intent(1, addr(9), e(2, 18), e(3000, 6)),
                addr(9),
                tid(),
                TradePrices::default(),
            )
            .await
            .unwrap();

        assert_eq!(out.status, TradeStatus::Submitted);
        let info = h.trades.info(&out.trade_id).await.unwrap().unwrap();
        assert_eq!(info.legs.len(), 1);
        assert_eq!(info.trade.taker, addr(9));
        // The payout was held.
        assert!(h.ledger.available(&virt(3, USDC)) < before);
    }

    /// The shape that made this a live defect: a swapper leg plus an interface fee, same token.
    /// Sourcing only the first leaves the fill short by the fee, which the contract discovers after
    /// it has already bought from every maker and spent the gas.
    #[tokio::test]
    async fn a_fee_leg_is_sourced_too() {
        let h = harness(vec![xyc(3, e(100, 18), e(300_000, 6))], SimVerdict::Ok).await;
        let mut order = intent(1, addr(9), e(2, 18), e(3000, 6));
        order.outputs.push(IntentOutput::new(
            addr(USDC),
            AmountCurve::scalar(e(25, 6)),
            addr(0xFE),
        ));

        let before = h.ledger.available(&virt(3, USDC));
        let out = h
            .swap
            .submit(order, addr(9), tid(), TradePrices::default())
            .await
            .unwrap();
        assert_eq!(out.status, TradeStatus::Submitted);

        // The hold covers both legs, not just the swapper's.
        let held = before - h.ledger.available(&virt(3, USDC));
        assert!(
            held >= e(3025, 6),
            "held {held} should cover the swapper leg plus the fee leg"
        );
    }

    /// Legs in different tokens each need their own route and reservation, all-or-nothing. No live
    /// order is shaped that way, so the swap path declines rather than under-sourcing.
    #[tokio::test]
    async fn outputs_in_two_tokens_are_declined() {
        let h = harness(vec![xyc(3, e(100, 18), e(300_000, 6))], SimVerdict::Ok).await;
        let mut order = intent(1, addr(9), e(2, 18), e(3000, 6));
        order.outputs.push(IntentOutput::new(
            addr(0xDA),
            AmountCurve::scalar(e(25, 18)),
            addr(0xFE),
        ));

        let out = h
            .swap
            .submit(order, addr(9), tid(), TradePrices::default())
            .await
            .unwrap();
        assert_eq!(out.status, TradeStatus::Declined);
    }

    #[tokio::test]
    async fn unroutable_order_is_declined() {
        // No makers ⇒ no route.
        let h = harness(vec![], SimVerdict::Ok).await;
        let out = h
            .swap
            .submit(
                intent(1, addr(9), e(2, 18), e(3000, 6)),
                addr(9),
                tid(),
                TradePrices::default(),
            )
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
        let first = h
            .swap
            .submit(order.clone(), addr(9), tid(), TradePrices::default())
            .await
            .unwrap();
        let held = h.ledger.available(&virt(3, USDC));
        // A second submit of the same order (new candidate id) neither re-reserves nor re-fills.
        let again = h
            .swap
            .submit(
                order,
                addr(9),
                TradeId(Ulid::from_parts(1_700_000_000_999, 2)),
                TradePrices::default(),
            )
            .await
            .unwrap();
        assert_eq!(again.trade_id, first.trade_id);
        assert_eq!(h.ledger.available(&virt(3, USDC)), held, "no second hold");
    }

    #[tokio::test]
    async fn resubmit_of_a_wedged_quoted_trade_redrives_to_submitted() {
        let h = harness(vec![xyc(3, e(100, 18), e(300_000, 6))], SimVerdict::Ok).await;
        let order = intent(7, addr(9), e(2, 18), e(3000, 6));
        // A crash between create and reserve strands the trade at Quoted, never reserved or filled.
        let stuck = Trade {
            id: tid(),
            order_hash: order.id,
            taker: addr(9),
            token_in: addr(WETH),
            token_out: addr(USDC),
            amount_in: e(2, 18),
            min_amount_out: e(3000, 6),
            amount_out: None,
            status: TradeStatus::Quoted,
            deadline_block: order.deadline,
            signature: Some(order.signature.clone()),
            price_impact_pct: None,
            surplus: None,
            tx_hash: None,
            block_number: None,
            created_at: 1_700_000_000,
            settled_at: None,
            token_in_price_usd: None,
            token_out_price_usd: None,
        };
        h.trades
            .create(
                &stuck,
                &[],
                &reached(1_700_000_000, &[TradeStatus::Created, TradeStatus::Quoted]),
            )
            .await
            .unwrap();

        // Resubmitting the same order (a new candidate id) re-drives the wedged trade forward
        // rather than echoing the stuck Quoted status.
        let out = h
            .swap
            .submit(
                order,
                addr(9),
                TradeId(Ulid::from_parts(1_700_000_000_999, 5)),
                TradePrices::default(),
            )
            .await
            .unwrap();
        assert_eq!(out.trade_id, stuck.id, "same trade, not a new one");
        assert_eq!(
            out.status,
            TradeStatus::Submitted,
            "the wedged trade advanced to submitted"
        );
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
            .submit(
                intent(1, addr(9), e(2, 18), e(3000, 6)),
                addr(9),
                tid(),
                TradePrices::default(),
            )
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
