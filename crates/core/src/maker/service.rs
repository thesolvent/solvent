//! The maker read-surface: turns shipped strategies into positions, the positions list, and the
//! roster — composing the registry snapshot, the balances oracle (pullable), USD valuation, the T1
//! range decoder, the T2 metrics store, and the event log (opening balances). The current curve spot
//! (`mid_price`) is filled by the marginal-price task.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use alloy_primitives::{Address, U256};
use itertools::Itertools;

use crate::asset::AssetManager;
use crate::deps::balances::BalancesOracle;
use crate::deps::ledger::Clock;
use crate::deps::maker_metrics::{MakerMetrics, MakerMetricsStore, PositionMetrics, TokenVolume};
use crate::deps::registry::EventStore;
use crate::primitives::amount::{Amount, Holdings, TokenAmounts};
use crate::primitives::asset::Token;
use crate::primitives::maker::{
    ActiveStats, Economics, FillShare, InventoryLeg, InventoryRow, MakerDashboard, MakerKpis,
    MakerSummary, Position, PositionBalances, PreviewResponse, PriceRange, Split,
};
use crate::primitives::pool::classify_pair;
use crate::primitives::registry::{
    curve_label, fee_in_bps, price_range, AquaEvent, Curve, CurveKind, CurveSpec, MakerStrategy,
    PositionRange, RangeKind, TokenPair,
};
use crate::primitives::{ChainId, MakerId, StrategyHash};
use crate::registry::SharedSnapshot;
use crate::valuation::Valuation;
use crate::SolventError;

const DAY: u64 = 86_400;
/// The activity window for economics and active-stats.
const WINDOW_DAYS: u64 = 7;
/// Basis points in one whole unit (100%); a fee in bps over this is the fraction taken.
const BPS_PER_UNIT: f64 = 10_000.0;

pub struct MakerService {
    registry: Arc<SharedSnapshot>,
    assets: Arc<AssetManager>,
    valuation: Arc<Valuation>,
    metrics: Arc<dyn MakerMetricsStore>,
    balances: Arc<dyn BalancesOracle>,
    events: Arc<dyn EventStore>,
    clock: Arc<dyn Clock>,
    chain: ChainId,
}

impl MakerService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        registry: Arc<SharedSnapshot>,
        assets: Arc<AssetManager>,
        valuation: Arc<Valuation>,
        metrics: Arc<dyn MakerMetricsStore>,
        balances: Arc<dyn BalancesOracle>,
        events: Arc<dyn EventStore>,
        clock: Arc<dyn Clock>,
        chain: ChainId,
    ) -> Self {
        Self {
            registry,
            assets,
            valuation,
            metrics,
            balances,
            events,
            clock,
            chain,
        }
    }

    /// Pre-flight a ship the SDK already encoded: whether the strategy already exists, which of
    /// `amounts` the maker's Aqua allowance can't yet cover (approve first), and any warnings. The
    /// SDK owns the encoding; the server only checks live chain state.
    pub async fn preview(
        &self,
        maker: Address,
        strategy_hash: StrategyHash,
        amounts: &[(Address, U256)],
    ) -> Result<PreviewResponse, SolventError> {
        let exists = self
            .registry
            .load()
            .strategy_by_hash(strategy_hash)
            .is_some();
        let tokens: Vec<Address> = amounts.iter().map(|(token, _)| *token).collect();
        let holdings = self.balances.holdings(maker, &tokens).await?;

        let mut requires_approval = Vec::new();
        let mut warnings = Vec::new();
        for (token, amount) in amounts {
            let held = holdings.get(token);
            let balance = held.map_or(U256::ZERO, |h| h.balance);
            let pullable = held.map_or(U256::ZERO, |h| h.pullable);
            if balance < *amount {
                warnings.push(format!("insufficient balance for {token}"));
            } else if pullable < *amount {
                requires_approval.push(*token);
            }
        }
        Ok(PreviewResponse {
            exists,
            requires_approval,
            warnings,
        })
    }

    /// One position by its strategy hash (active or docked), with detail stats — or `None` if unknown
    /// or unpriceable.
    pub async fn position(&self, hash: StrategyHash) -> Result<Option<Position>, SolventError> {
        let snapshot = self.registry.load();
        let Some(strategy) = snapshot.strategy_by_hash(hash).cloned() else {
            return Ok(None);
        };
        self.build(&strategy, true).await
    }

    /// A maker's active positions (list projection — no detail stats).
    pub async fn positions(&self, maker: MakerId) -> Result<Vec<Position>, SolventError> {
        let strategies: Vec<MakerStrategy> = self
            .registry
            .load()
            .strategies_for_maker(maker)
            .cloned()
            .collect();
        let mut positions = Vec::new();
        for strategy in &strategies {
            if let Some(position) = self.build(strategy, false).await? {
                positions.push(position);
            }
        }
        Ok(positions)
    }

    /// The active-maker roster: each maker's active-position count and shared liquidity.
    pub async fn roster(&self) -> Result<Vec<MakerSummary>, SolventError> {
        // Group active strategies by maker → (count, per-token committed balances).
        let grouped: HashMap<MakerId, (u64, Vec<(Address, U256)>)> = self
            .registry
            .load()
            .active_strategies()
            .filter(|s| matches!(s.curve, CurveSpec::Priceable { .. }))
            .map(|s| (s.key.maker, s))
            .into_grouping_map()
            .fold((0u64, Vec::new()), |(count, mut bals), _maker, s| {
                bals.extend(s.balances.iter().map(|(a, v)| (*a, *v)));
                (count + 1, bals)
            });

        let mut roster = Vec::with_capacity(grouped.len());
        for (maker, (active_position_count, balances)) in grouped {
            let priced: Vec<(Token, U256)> = balances
                .into_iter()
                .map(|(addr, amt)| (self.assets.token_or_default(addr), amt))
                .collect();
            roster.push(MakerSummary {
                maker: maker.0,
                active_positions: active_position_count,
                shared_liquidity_usd: self.valuation.tvl_usd(&priced).await,
            });
        }
        roster.sort_by_key(|entry| entry.maker);
        Ok(roster)
    }

    /// One maker's dashboard: headline KPIs with period-over-period deltas, fill-share on its pairs,
    /// latency, and a competitive insight. Composed at the producer (all USD valued here).
    pub async fn dashboard(&self, maker: MakerId) -> Result<MakerDashboard, SolventError> {
        let now = self.clock.now_unix();
        let window = WINDOW_DAYS * DAY;

        // Headline totals reuse the position builder, so the dashboard can't drift from the list.
        let positions = self.positions(maker).await?;
        let kpis = position_kpis(&positions);

        let strategies = self.active_priceable_strategies(maker);
        let wallet_balance_usd = self
            .wallet_usd(maker.0, &strategy_tokens(&strategies))
            .await?;

        let metrics = self.window_metrics(maker, now, window).await?;
        let deltas = self
            .window_deltas(&metrics, kpis.volume_usd, kpis.shared_liquidity_usd)
            .await;
        let fill_share = self
            .fill_share(
                &strategies,
                metrics.current.fills,
                now.saturating_sub(window),
            )
            .await?;

        Ok(MakerDashboard {
            maker: maker.0,
            window_days: WINDOW_DAYS as u32,
            active_positions: positions.len() as u64,
            kpis: MakerKpis {
                shared_liquidity_usd: kpis.shared_liquidity_usd,
                volume_usd: kpis.volume_usd,
                wallet_balance_usd,
                pullable_usd: kpis.pullable_usd,
                shared_liq_ratio: ratio(kpis.pullable_usd, kpis.shared_liquidity_usd),
                fees_usd: kpis.fees_usd,
                shared_liquidity_change_pct: deltas.shared_liquidity_change_pct,
                volume_change_pct: deltas.volume_change_pct,
                fees_change_pct: deltas.fees_change_pct,
            },
            fill_share,
            latency_p50_ms: metrics.current.latency_p50_ms,
            insight: self.undercut_insight(maker, &strategies),
        })
    }

    /// The maker's active, priceable strategies — the set the wallet, fill-share, and insight derive
    /// from.
    fn active_priceable_strategies(&self, maker: MakerId) -> Vec<MakerStrategy> {
        self.registry
            .load()
            .strategies_for_maker(maker)
            .filter(|s| s.active && matches!(s.curve, CurveSpec::Priceable { .. }))
            .cloned()
            .collect()
    }

    /// The current window's metrics and the previous equal window's, for period-over-period deltas.
    async fn window_metrics(
        &self,
        maker: MakerId,
        now: u64,
        window: u64,
    ) -> Result<WindowMetrics, SolventError> {
        let current = self
            .metrics
            .maker(maker, now.saturating_sub(window), now)
            .await?;
        let previous = self
            .metrics
            .maker(
                maker,
                now.saturating_sub(2 * window),
                now.saturating_sub(window),
            )
            .await?;
        Ok(WindowMetrics { current, previous })
    }

    /// Period-over-period KPI deltas: volume vs the previous window, fees tracking volume, and the
    /// shared-liquidity change from net trading flow.
    async fn window_deltas(
        &self,
        metrics: &WindowMetrics,
        volume_usd: Option<f64>,
        shared_liquidity_usd: Option<f64>,
    ) -> WindowDeltas {
        let volume_change_pct = change_pct(volume_usd, self.usd_of(&metrics.previous.volume).await);
        WindowDeltas {
            volume_change_pct,
            // Fees track volume at a stable fee mix, so the fee delta mirrors the volume delta.
            fees_change_pct: volume_change_pct,
            shared_liquidity_change_pct: self
                .liquidity_change(&metrics.current, shared_liquidity_usd)
                .await,
        }
    }

    /// The maker's fills as a share of all fills on the pairs it quotes.
    async fn fill_share(
        &self,
        strategies: &[MakerStrategy],
        filled: u64,
        since: u64,
    ) -> Result<FillShare, SolventError> {
        let pairs: Vec<TokenPair> = strategies
            .iter()
            .filter_map(|s| s.pair())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let pair_fills = self.metrics.pair_fills(&pairs, since).await?;
        Ok(FillShare {
            filled,
            pair_fills,
            share_pct: pct_of(filled, pair_fills),
        })
    }

    /// Raw wallet USD across `tokens` (uncapped `balance`, not the pullable-capped `actual`).
    async fn wallet_usd(
        &self,
        maker: Address,
        tokens: &[Address],
    ) -> Result<Option<f64>, SolventError> {
        let holdings = self.balances.holdings(maker, tokens).await?;
        let priced: Vec<_> = holdings
            .iter()
            .map(|(addr, h)| (self.assets.token_or_default(*addr), h.balance))
            .collect();
        Ok(self.valuation.tvl_usd(&priced).await)
    }

    /// Change in shared liquidity attributable to trading: net USD flow (`inflow − outflow`) over the
    /// window, against the reconstructed start-of-window liquidity. Assumes trades are the in-window
    /// driver (ship/dock/push mid-window not modeled).
    async fn liquidity_change(&self, cur: &MakerMetrics, shared_usd: Option<f64>) -> Option<f64> {
        let net = self.usd_of(&cur.inflow).await? - self.usd_of(&cur.volume).await?;
        let start = shared_usd? - net;
        (start > 0.0).then(|| net / start * 100.0)
    }

    /// A cheaper competitor on one of the maker's pairs — the largest fee undercut by another maker,
    /// or `None` if nobody undercuts it.
    fn undercut_insight(&self, maker: MakerId, strategies: &[MakerStrategy]) -> Option<String> {
        let snapshot = self.registry.load();
        let mut best: Option<Undercut> = None;
        for mine in strategies.iter().filter_map(priceable_fee) {
            for other in snapshot.active_strategies() {
                if other.key.maker == maker {
                    continue;
                }
                let Some(theirs) = priceable_fee(other) else {
                    continue;
                };
                if theirs.pair != mine.pair {
                    continue;
                }
                let gap_bps = mine.fee_bps.saturating_sub(theirs.fee_bps);
                if gap_bps > 0 && best.as_ref().is_none_or(|b| gap_bps > b.gap_bps) {
                    best = Some(Undercut {
                        gap_bps,
                        pair: mine.pair,
                        competitor: other.key.maker.0,
                        their_fee_bps: theirs.fee_bps,
                        my_fee_bps: mine.fee_bps,
                    });
                }
            }
        }
        best.map(|u| {
            format!(
                "{} quotes {} at {} bps vs your {} bps",
                short_addr(u.competitor),
                self.assets.pair_label(&u.pair),
                u.their_fee_bps,
                u.my_fee_bps
            )
        })
    }

    /// The maker's positions re-grouped by token — the Assets tab. Each token row aggregates the
    /// positions holding it (its `legs`), with wallet vs committed balances and economics.
    pub async fn inventory(&self, maker: MakerId) -> Result<Vec<InventoryRow>, SolventError> {
        let groups = self.group_legs_by_token(maker).await?;
        let tokens: Vec<Address> = groups.keys().copied().collect();
        let holdings = self.balances.holdings(maker.0, &tokens).await?;
        let mut rows = Vec::with_capacity(groups.len());
        for (addr, group) in groups {
            rows.push(self.inventory_row(addr, group, &holdings).await);
        }
        Ok(rows)
    }

    /// Rebuild the maker's detail positions and transpose their per-token legs into one group per
    /// token (a token can appear in several positions).
    async fn group_legs_by_token(
        &self,
        maker: MakerId,
    ) -> Result<BTreeMap<Address, TokenGroup>, SolventError> {
        let strategies: Vec<MakerStrategy> = self
            .registry
            .load()
            .strategies_for_maker(maker)
            .cloned()
            .collect();
        let mut groups: BTreeMap<Address, TokenGroup> = BTreeMap::new();
        for strategy in &strategies {
            let Some(position) = self.build(strategy, true).await? else {
                continue;
            };
            for entry in &position.balances.virtual_balances.entries {
                let addr = entry.token.address;
                let opening = find_amount(&position.balances.opening, addr)
                    .cloned()
                    .unwrap_or_else(|| Amount::from_base_units(U256::ZERO, entry.token.decimals));
                groups
                    .entry(addr)
                    .or_insert_with(|| TokenGroup {
                        token: entry.token.clone(),
                        legs: Vec::new(),
                    })
                    .legs
                    .push(InventoryLeg {
                        pair: position.pair.clone(),
                        curve: position.curve.clone(),
                        fee_bps: position.fee_bps,
                        current: entry.amount.clone(),
                        opening,
                        volume_usd: position.economics.volume_usd,
                        fees_usd: position.economics.fees_usd,
                        apy_pct: position.economics.apy_pct,
                        coverage: position.balances.coverage,
                    });
            }
        }
        Ok(groups)
    }

    /// One inventory row: the token's wallet holding, its committed (shared) total across legs, and
    /// the fees/APY those legs earned.
    async fn inventory_row(
        &self,
        addr: Address,
        group: TokenGroup,
        holdings: &BTreeMap<Address, Holdings>,
    ) -> InventoryRow {
        let TokenGroup { token, legs } = group;
        let wallet_raw = holdings.get(&addr).map_or(U256::ZERO, |h| h.balance);
        let wallet = self
            .valuation
            .amount(wallet_raw, addr, token.decimals)
            .await;
        let shared_raw = legs
            .iter()
            .filter_map(|leg| leg.current.raw.parse::<U256>().ok())
            .fold(U256::ZERO, U256::saturating_add);
        let shared = self
            .valuation
            .amount(shared_raw, addr, token.decimals)
            .await;
        let fees_usd = sum_usd(legs.iter().map(|leg| leg.fees_usd));
        let apy_pct = annualized_apy(fees_usd, shared.usd);
        InventoryRow {
            token,
            wallet,
            shared,
            fees_usd,
            apy_pct,
            legs,
        }
    }

    async fn build(
        &self,
        strategy: &MakerStrategy,
        detail: bool,
    ) -> Result<Option<Position>, SolventError> {
        let CurveSpec::Priceable { curve, fees_in_bps } = &strategy.curve else {
            return Ok(None);
        };
        let Some(pair) = strategy.pair() else {
            return Ok(None);
        };
        let (base_addr, quote_addr) = self.assets.base_quote(&pair);
        let (Some(base), Some(quote)) = (
            self.assets.token(&base_addr),
            self.assets.token(&quote_addr),
        ) else {
            return Ok(None);
        };
        let fee_bps = fee_in_bps(fees_in_bps);
        let range = self.range(curve, &pair, base_addr == pair.lo, &quote);

        let virtual_balances = self.valued(&strategy.balances).await;
        let actual = self.pullable(strategy).await?;
        let shortfall = self.shortfall(&strategy.balances, &actual);
        let coverage = coverage(&actual, &virtual_balances);
        // Opening balances read the ship-time event log — detail only (off the list hot path).
        let opening = if detail {
            self.opening(strategy.key.strategy_hash).await?
        } else {
            TokenAmounts {
                entries: Vec::new(),
                total_usd: None,
            }
        };

        let metrics = self
            .metrics
            .position(strategy.key.strategy_hash, pair, self.since())
            .await?;
        let volume_usd = self.usd_volume(&metrics).await;
        let economics = economics(volume_usd, fee_bps, virtual_balances.total_usd);

        Ok(Some(Position {
            strategy_hash: format!("{:#x}", strategy.key.strategy_hash.0),
            pair: self.assets.pair_label(&pair),
            base,
            quote,
            maker: strategy.key.maker.0,
            curve: curve_label(curve).to_string(),
            pair_type: classify_pair(
                self.assets.is_stable(&pair.lo),
                self.assets.is_stable(&pair.hi),
                Some(curve_kind(curve)),
            ),
            state: if strategy.active { "active" } else { "docked" }.to_string(),
            fee_bps,
            mid_price: None,
            range,
            balances: PositionBalances {
                split: split(&virtual_balances),
                backed: shortfall.is_none(),
                shortfall,
                coverage,
                opening,
                virtual_balances,
                actual,
            },
            economics,
            active_stats: detail.then_some(ActiveStats {
                fills_7d: metrics.fills,
                volume_7d_usd: volume_usd,
                quote_uptime_pct: metrics.quote_uptime_pct,
                last_fill_at: metrics.last_fill_at,
            }),
        }))
    }

    /// A `TokenAmounts` from committed balances, valued (all-or-nothing total).
    async fn valued(&self, balances: &BTreeMap<Address, U256>) -> TokenAmounts {
        let holdings: Vec<(Token, U256)> = balances
            .iter()
            .map(|(addr, amount)| (self.assets.token_or_default(*addr), *amount))
            .collect();
        self.valuation.priced_amounts(&holdings).await
    }

    /// The pullable (`min(committed, on-chain pullable)`) balances for a strategy, valued.
    async fn pullable(&self, strategy: &MakerStrategy) -> Result<TokenAmounts, SolventError> {
        let tokens: Vec<Address> = strategy.balances.keys().copied().collect();
        let holdings = self
            .balances
            .holdings(strategy.key.maker.0, &tokens)
            .await?;
        let capped: BTreeMap<Address, U256> = strategy
            .balances
            .iter()
            .map(|(addr, committed)| {
                let pullable = holdings.get(addr).map_or(U256::ZERO, |h| h.pullable);
                (*addr, (*committed).min(pullable))
            })
            .collect();
        Ok(self.valued(&capped).await)
    }

    /// A human deficit string for tokens the wallet under-backs (`committed − pullable`), or `None`
    /// when every token is fully backed (which is what `backed` means). E.g. `"short 37600 USDC"`.
    fn shortfall(
        &self,
        committed: &BTreeMap<Address, U256>,
        actual: &TokenAmounts,
    ) -> Option<String> {
        let deficits: Vec<String> = committed
            .iter()
            .filter_map(|(addr, want)| {
                let have = actual
                    .entries
                    .iter()
                    .find(|e| e.token.address == *addr)
                    .and_then(|e| e.amount.raw.parse::<U256>().ok())
                    .unwrap_or(U256::ZERO);
                let deficit = want.saturating_sub(have);
                (!deficit.is_zero()).then(|| {
                    let token = self.assets.token_or_default(*addr);
                    format!(
                        "{} {}",
                        Amount::from_base_units(deficit, token.decimals).display,
                        token.symbol
                    )
                })
            })
            .collect();
        (!deficits.is_empty()).then(|| format!("short {}", deficits.join(" · ")))
    }

    /// The strategy's opening balances — the `Pushed` legs at its ship block — valued. Empty if the
    /// ship isn't in the log yet.
    async fn opening(&self, hash: StrategyHash) -> Result<TokenAmounts, SolventError> {
        let history = self.events.history(self.chain, hash).await?;
        // A strategy first appears at its ship block, and `ship()` emits its opening deposit as
        // `Pushed` legs in that block; swaps arrive in later blocks, so the first block's pushes
        // are exactly the opening.
        let Some(ship_block) = history.iter().filter_map(|e| e.block_number).min() else {
            return Ok(TokenAmounts {
                entries: Vec::new(),
                total_usd: None,
            });
        };
        let mut opening: BTreeMap<Address, U256> = BTreeMap::new();
        for e in &history {
            if e.block_number == Some(ship_block) {
                if let AquaEvent::Pushed { token, amount, .. } = &e.event {
                    *opening.entry(*token).or_default() += *amount;
                }
            }
        }
        Ok(self.valued(&opening).await)
    }

    async fn usd_volume(&self, metrics: &PositionMetrics) -> Option<f64> {
        self.usd_of(&metrics.volume).await
    }

    /// The USD value of a per-token flow (all-or-nothing — `None` if any token is unpriced).
    async fn usd_of(&self, volume: &[TokenVolume]) -> Option<f64> {
        let priced: Vec<_> = volume
            .iter()
            .map(|v| (self.assets.token_or_default(v.token), v.base_units))
            .collect();
        self.valuation.tvl_usd(&priced).await
    }

    fn range(
        &self,
        curve: &Curve,
        pair: &TokenPair,
        base_is_lo: bool,
        quote: &crate::primitives::asset::Token,
    ) -> PriceRange {
        let dec_lo = self.assets.decimals(&pair.lo);
        let dec_hi = self.assets.decimals(&pair.hi);
        wire_range(
            price_range(curve, dec_lo, dec_hi, base_is_lo),
            &quote.symbol,
        )
    }

    fn since(&self) -> u64 {
        self.clock.now_unix().saturating_sub(WINDOW_DAYS * DAY)
    }
}

/// The dashboard's headline totals, folded from the position list.
struct PositionKpis {
    shared_liquidity_usd: Option<f64>,
    pullable_usd: Option<f64>,
    volume_usd: Option<f64>,
    fees_usd: Option<f64>,
}

/// A metrics window and the equal window before it, for period-over-period deltas.
struct WindowMetrics {
    current: MakerMetrics,
    previous: MakerMetrics,
}

/// The dashboard's period-over-period KPI deltas.
struct WindowDeltas {
    volume_change_pct: Option<f64>,
    fees_change_pct: Option<f64>,
    shared_liquidity_change_pct: Option<f64>,
}

/// A token's legs across the maker's positions — the Assets-tab transpose of one token.
struct TokenGroup {
    token: Token,
    legs: Vec<InventoryLeg>,
}

/// A priceable strategy's pair and fee, in real bps.
struct PairFee {
    pair: TokenPair,
    fee_bps: u32,
}

/// The largest fee undercut of a maker on one of its pairs by a competitor.
struct Undercut {
    gap_bps: u32,
    pair: TokenPair,
    competitor: Address,
    their_fee_bps: u32,
    my_fee_bps: u32,
}

/// The dashboard's headline totals from the position list (each all-or-nothing).
fn position_kpis(positions: &[Position]) -> PositionKpis {
    PositionKpis {
        shared_liquidity_usd: sum_usd(
            positions
                .iter()
                .map(|p| p.balances.virtual_balances.total_usd),
        ),
        pullable_usd: sum_usd(positions.iter().map(|p| p.balances.actual.total_usd)),
        volume_usd: sum_usd(positions.iter().map(|p| p.economics.volume_usd)),
        fees_usd: sum_usd(positions.iter().map(|p| p.economics.fees_usd)),
    }
}

/// The distinct tokens a set of strategies commits balances to.
fn strategy_tokens(strategies: &[MakerStrategy]) -> Vec<Address> {
    strategies
        .iter()
        .flat_map(|s| s.balances.keys().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// A priceable strategy's pair and fee in real bps, or `None` if it is unpriceable or has no pair.
fn priceable_fee(strategy: &MakerStrategy) -> Option<PairFee> {
    let CurveSpec::Priceable { fees_in_bps, .. } = &strategy.curve else {
        return None;
    };
    Some(PairFee {
        pair: strategy.pair()?,
        fee_bps: fee_in_bps(fees_in_bps),
    })
}

/// The `Amount` held for `token` among `amounts`, if present.
fn find_amount(amounts: &TokenAmounts, token: Address) -> Option<&Amount> {
    amounts
        .entries
        .iter()
        .find(|e| e.token.address == token)
        .map(|e| &e.amount)
}

/// USD economics from window volume, the strategy's fee, and committed liquidity. APY annualizes the
/// window's fees over the committed liquidity: `fees/liquidity · 365/window`.
fn economics(volume_usd: Option<f64>, fee_bps: u32, liquidity_usd: Option<f64>) -> Economics {
    let fees_usd = volume_usd.map(|v| v * f64::from(fee_bps) / BPS_PER_UNIT);
    Economics {
        apy_pct: annualized_apy(fees_usd, liquidity_usd),
        fees_usd,
        volume_usd,
    }
}

/// Annualize a window's fees over the liquidity that earned them: `fees/liq · 365/window · 100`.
fn annualized_apy(fees_usd: Option<f64>, liquidity_usd: Option<f64>) -> Option<f64> {
    match (fees_usd, liquidity_usd) {
        (Some(fees), Some(liq)) if liq > 0.0 => {
            Some(fees / liq * (365.0 / WINDOW_DAYS as f64) * 100.0)
        }
        _ => None,
    }
}

/// Backing coverage: pullable USD as a fraction of committed USD (`1.0` = fully backed). `None` when
/// either side is unpriced or committed is not positive.
fn coverage(actual: &TokenAmounts, virtual_balances: &TokenAmounts) -> Option<f64> {
    ratio(actual.total_usd, virtual_balances.total_usd)
}

/// Sum USD figures all-or-nothing: `None` if any is `None` (an unpriced part makes the total a guess).
fn sum_usd(values: impl IntoIterator<Item = Option<f64>>) -> Option<f64> {
    values.into_iter().try_fold(0.0, |acc, v| Some(acc + v?))
}

/// `num ÷ den` as a fraction, or `None` when either is missing or `den` is not positive.
fn ratio(num: Option<f64>, den: Option<f64>) -> Option<f64> {
    match (num, den) {
        (Some(n), Some(d)) if d > 0.0 => Some(n / d),
        _ => None,
    }
}

/// Period-over-period change: `(cur − prev) / prev · 100`, or `None` when either is missing or the
/// previous window has no value to compare against.
fn change_pct(cur: Option<f64>, prev: Option<f64>) -> Option<f64> {
    match (cur, prev) {
        (Some(c), Some(p)) if p > 0.0 => Some((c - p) / p * 100.0),
        _ => None,
    }
}

/// `a ÷ b` as a percentage, or `None` when `b` is zero.
fn pct_of(a: u64, b: u64) -> Option<f64> {
    (b > 0).then(|| a as f64 / b as f64 * 100.0)
}

/// A short `0x1234…abcd` form for an address in human-facing text.
fn short_addr(addr: Address) -> String {
    let hex = format!("{addr:#x}");
    format!("{}…{}", &hex[..6], &hex[hex.len() - 4..])
}

/// Per-token share of committed liquidity by USD value; empty if any token is unpriced.
fn split(balances: &TokenAmounts) -> Vec<Split> {
    let total: f64 = balances.total_usd.unwrap_or(0.0);
    if total <= 0.0 {
        return Vec::new();
    }
    balances
        .entries
        .iter()
        .filter_map(|entry| {
            entry.amount.usd.map(|usd| Split {
                token: entry.token.clone(),
                pct: usd / total * 100.0,
            })
        })
        .collect()
}

/// Map a decoded `PositionRange` to the wire shape, composing the human label with the quote symbol.
fn wire_range(range: PositionRange, quote_symbol: &str) -> PriceRange {
    let (kind, label) = match range.kind {
        RangeKind::Full => ("full", "Full range".to_string()),
        RangeKind::Bounded => (
            "bounded",
            match (&range.lower_price, &range.upper_price) {
                (Some(lo), Some(hi)) => format!("{lo}–{hi} {quote_symbol}"),
                _ => "Bounded".to_string(),
            },
        ),
        RangeKind::Peg => (
            "peg",
            match (&range.peg_price, range.below_pct) {
                (Some(peg), Some(pct)) => format!("±{pct:.2}% around {peg} {quote_symbol}"),
                (Some(peg), None) => format!("{peg} {quote_symbol}"),
                _ => "Pegged".to_string(),
            },
        ),
    };
    PriceRange {
        kind: kind.to_string(),
        lower_price: range.lower_price,
        upper_price: range.upper_price,
        peg_price: range.peg_price,
        below_pct: range.below_pct,
        above_pct: range.above_pct,
        label,
    }
}

fn curve_kind(curve: &Curve) -> CurveKind {
    match curve {
        Curve::Xyc => CurveKind::Xyc,
        Curve::Concentrate { .. } => CurveKind::Concentrated,
        Curve::Pegged(_) => CurveKind::Pegged,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use alloy_primitives::B256;
    use async_trait::async_trait;
    use rust_decimal::Decimal;

    use super::*;
    use crate::deps::balances::BalancesOracleError;
    use crate::deps::maker_metrics::{MakerMetrics, MakerMetricsError, PairMetrics, TokenVolume};
    use crate::deps::registry::{RecordedEvent, StoreError};
    use crate::deps::routing::{PriceOracle, PriceOracleError};
    use crate::primitives::amount::Holdings;
    use crate::primitives::asset::{TokenList, TokenMeta};
    use crate::primitives::registry::{Curve, EventCursor, EventExt, Snapshot, StrategyKey};
    use crate::primitives::UsdPrice;

    const USDC: u8 = 2; // lower address → the stable quote
    const WETH: u8 = 3;
    const USDT: u8 = 4;

    fn addr(n: u8) -> Address {
        Address::from([n; 20])
    }

    struct FakePrices(HashMap<Address, u64>);
    #[async_trait]
    impl PriceOracle for FakePrices {
        async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError> {
            self.0
                .get(&token)
                .map(|d| UsdPrice(Decimal::from(*d)))
                .ok_or(PriceOracleError::NotFound(token))
        }
    }

    struct FakeOracle(HashMap<Address, U256>);
    #[async_trait]
    impl BalancesOracle for FakeOracle {
        async fn holdings(
            &self,
            _: Address,
            tokens: &[Address],
        ) -> Result<BTreeMap<Address, Holdings>, BalancesOracleError> {
            Ok(tokens
                .iter()
                .filter_map(|t| {
                    self.0.get(t).map(|pullable| {
                        (
                            *t,
                            Holdings {
                                balance: *pullable,
                                pullable: *pullable,
                            },
                        )
                    })
                })
                .collect())
        }
    }

    struct FakeMetrics;
    #[async_trait]
    impl MakerMetricsStore for FakeMetrics {
        async fn maker(
            &self,
            _: MakerId,
            _: u64,
            now: u64,
        ) -> Result<MakerMetrics, MakerMetricsError> {
            // Current window (now = clock) delivers 1 WETH ($2000) and receives 2010 USDC (net +$10);
            // the previous window (earlier `now`) delivered only 0.5 WETH ($1000) → +100% volume.
            let current = now >= 1_000_000;
            Ok(MakerMetrics {
                fills: if current { 20 } else { 0 },
                fills_by_day: [0; 7],
                last_fill_at: Some(1_700),
                volume: vec![TokenVolume {
                    token: addr(WETH),
                    base_units: U256::from(if current {
                        1_000_000_000_000_000_000u128
                    } else {
                        500_000_000_000_000_000u128
                    }),
                }],
                inflow: if current {
                    vec![TokenVolume {
                        token: addr(USDC),
                        base_units: U256::from(2_010_000_000u64), // 2010 USDC
                    }]
                } else {
                    Vec::new()
                },
                quotes: 40,
                latency_p50_ms: Some(120),
            })
        }
        async fn position(
            &self,
            _: StrategyHash,
            _: TokenPair,
            _: u64,
        ) -> Result<PositionMetrics, MakerMetricsError> {
            Ok(PositionMetrics {
                fills: 5,
                volume: vec![TokenVolume {
                    token: addr(WETH),
                    base_units: U256::from(1_000_000_000_000_000_000u128), // 1 WETH
                }],
                last_fill_at: Some(1_700),
                quote_uptime_pct: Some(75.0),
            })
        }
        async fn pair_activity(
            &self,
            _: TokenPair,
            _: u64,
        ) -> Result<PairMetrics, MakerMetricsError> {
            Ok(PairMetrics {
                fills: 0,
                volume: vec![],
            })
        }

        async fn pair_fills(&self, _: &[TokenPair], _: u64) -> Result<u64, MakerMetricsError> {
            Ok(50)
        }
    }

    struct FixedClock;
    impl Clock for FixedClock {
        fn now_unix(&self) -> u64 {
            1_000_000
        }
    }

    /// A ship at block 100 (1 WETH + 2000 USDC opening) followed by a later swap inflow at block 200
    /// — the opening reader must count only the ship-block legs.
    struct FakeEvents;
    #[async_trait]
    impl EventStore for FakeEvents {
        async fn cursor(&self, _: ChainId) -> Result<Option<EventCursor>, StoreError> {
            Ok(None)
        }
        async fn insert(
            &self,
            _: ChainId,
            _: &[EventExt<AquaEvent>],
        ) -> Result<Vec<EventExt<AquaEvent>>, StoreError> {
            Ok(Vec::new())
        }
        async fn save_cursor(&self, _: ChainId, _: EventCursor) -> Result<(), StoreError> {
            Ok(())
        }
        async fn events(&self, _: ChainId) -> Result<Vec<EventExt<AquaEvent>>, StoreError> {
            Ok(Vec::new())
        }
        async fn history(
            &self,
            _: ChainId,
            hash: StrategyHash,
        ) -> Result<Vec<EventExt<AquaEvent>>, StoreError> {
            let push = |token, amount, block| EventExt {
                event: AquaEvent::Pushed {
                    maker: MakerId(Address::ZERO),
                    app: Address::ZERO,
                    strategy_hash: hash,
                    token,
                    amount,
                },
                address: Address::ZERO,
                block_hash: None,
                block_number: Some(block),
                transaction_hash: None,
                transaction_index: None,
                log_index: None,
                removed: false,
            };
            Ok(vec![
                push(addr(WETH), U256::from(1_000_000_000_000_000_000u128), 100),
                push(addr(USDC), U256::from(2_000_000_000u64), 100),
                push(addr(USDC), U256::from(500_000_000u64), 200), // later swap — excluded
            ])
        }
        async fn recent(
            &self,
            _: ChainId,
            _: Option<EventCursor>,
            _: u32,
        ) -> Result<Vec<RecordedEvent>, StoreError> {
            Ok(Vec::new())
        }
        async fn count_since(&self, _: ChainId, _: u64) -> Result<u64, StoreError> {
            Ok(0)
        }
    }

    fn xyc_strategy(maker: u8) -> MakerStrategy {
        let key = StrategyKey {
            maker: MakerId(addr(maker)),
            app: Address::ZERO,
            strategy_hash: StrategyHash(B256::from([maker; 32])),
        };
        let mut st = MakerStrategy::new(key, &[]);
        st.curve = CurveSpec::Priceable {
            curve: Curve::Xyc,
            fees_in_bps: vec![500_000], // 5 real bps
        };
        st.balances
            .insert(addr(WETH), U256::from(1_000_000_000_000_000_000u128)); // 1 WETH
        st.balances.insert(addr(USDC), U256::from(2_000_000_000u64)); // 2000 USDC
        st
    }

    /// A USDC/USDT strategy (both 6-decimal stables) for the same maker — shares USDC with the
    /// WETH/USDC position, so inventory grouping puts two legs under the USDC row.
    fn usdc_usdt_strategy(maker: u8, hash: u8) -> MakerStrategy {
        let key = StrategyKey {
            maker: MakerId(addr(maker)),
            app: Address::ZERO,
            strategy_hash: StrategyHash(B256::from([hash; 32])),
        };
        let mut st = MakerStrategy::new(key, &[]);
        st.curve = CurveSpec::Priceable {
            curve: Curve::Xyc,
            fees_in_bps: vec![100_000], // 1 real bp
        };
        st.balances.insert(addr(USDC), U256::from(1_000_000_000u64)); // 1000 USDC
        st.balances.insert(addr(USDT), U256::from(1_000_000_000u64)); // 1000 USDT
        st
    }

    fn service(snapshot: Snapshot, pullable: HashMap<Address, U256>) -> MakerService {
        service_with(snapshot, Arc::new(FakeOracle(pullable)))
    }

    fn service_with(snapshot: Snapshot, balances: Arc<dyn BalancesOracle>) -> MakerService {
        let registry = Arc::new(SharedSnapshot::new(snapshot));
        let list = TokenList {
            name: "t".into(),
            tokens: vec![
                TokenMeta {
                    chain_id: 1,
                    address: addr(USDC),
                    symbol: "USDC".into(),
                    name: "USDC".into(),
                    decimals: 6,
                    logo_uri: None,
                    tags: vec!["stables".into()],
                },
                TokenMeta {
                    chain_id: 1,
                    address: addr(WETH),
                    symbol: "WETH".into(),
                    name: "WETH".into(),
                    decimals: 18,
                    logo_uri: None,
                    tags: vec![],
                },
                TokenMeta {
                    chain_id: 1,
                    address: addr(USDT),
                    symbol: "USDT".into(),
                    name: "USDT".into(),
                    decimals: 6,
                    logo_uri: None,
                    tags: vec!["stables".into()],
                },
            ],
        };
        let assets = Arc::new(AssetManager::new(list, Arc::clone(&registry)));
        let valuation = Arc::new(Valuation::new(Arc::new(FakePrices(HashMap::from([
            (addr(WETH), 2000),
            (addr(USDC), 1),
            (addr(USDT), 1),
        ])))));
        MakerService::new(
            registry,
            assets,
            valuation,
            Arc::new(FakeMetrics),
            balances,
            Arc::new(FakeEvents),
            Arc::new(FixedClock),
            ChainId(1),
        )
    }

    struct PreviewOracle(HashMap<Address, Holdings>);
    #[async_trait]
    impl BalancesOracle for PreviewOracle {
        async fn holdings(
            &self,
            _: Address,
            tokens: &[Address],
        ) -> Result<BTreeMap<Address, Holdings>, BalancesOracleError> {
            Ok(tokens
                .iter()
                .filter_map(|t| {
                    self.0.get(t).map(|h| {
                        (
                            *t,
                            Holdings {
                                balance: h.balance,
                                pullable: h.pullable,
                            },
                        )
                    })
                })
                .collect())
        }
    }

    #[tokio::test]
    async fn preview_flags_an_existing_strategy() {
        let svc = service(Snapshot::from_strategies([xyc_strategy(9)]), HashMap::new());
        let out = svc
            .preview(addr(9), StrategyHash(B256::from([9; 32])), &[])
            .await
            .unwrap();
        assert!(out.exists);
        assert!(out.requires_approval.is_empty());
        assert!(out.warnings.is_empty());
    }

    #[tokio::test]
    async fn preview_requires_approval_when_allowance_is_short() {
        // Holds 100 WETH but only 10 is pullable (allowance-capped); shipping 50 needs an approval.
        let oracle = PreviewOracle(HashMap::from([(
            addr(WETH),
            Holdings {
                balance: U256::from(100u64),
                pullable: U256::from(10u64),
            },
        )]));
        let svc = service_with(
            Snapshot::from_strategies([xyc_strategy(9)]),
            Arc::new(oracle),
        );
        let out = svc
            .preview(
                addr(1),
                StrategyHash(B256::from([7; 32])),
                &[(addr(WETH), U256::from(50u64))],
            )
            .await
            .unwrap();
        assert!(!out.exists);
        assert_eq!(out.requires_approval, vec![addr(WETH)]);
        assert!(out.warnings.is_empty());
    }

    #[tokio::test]
    async fn preview_warns_on_insufficient_balance() {
        let oracle = PreviewOracle(HashMap::from([(
            addr(WETH),
            Holdings {
                balance: U256::from(5u64),
                pullable: U256::from(5u64),
            },
        )]));
        let svc = service_with(
            Snapshot::from_strategies([xyc_strategy(9)]),
            Arc::new(oracle),
        );
        let out = svc
            .preview(
                addr(1),
                StrategyHash(B256::from([7; 32])),
                &[(addr(WETH), U256::from(50u64))],
            )
            .await
            .unwrap();
        assert!(out
            .warnings
            .iter()
            .any(|w| w.contains("insufficient balance")));
        assert!(out.requires_approval.is_empty());
    }

    #[tokio::test]
    async fn builds_a_position() {
        // USDC fully pullable; WETH only half → not fully backed.
        let pullable = HashMap::from([
            (addr(WETH), U256::from(500_000_000_000_000_000u128)),
            (addr(USDC), U256::from(2_000_000_000u64)),
        ]);
        let svc = service(Snapshot::from_strategies([xyc_strategy(9)]), pullable);

        let p = svc
            .position(StrategyHash(B256::from([9; 32])))
            .await
            .unwrap()
            .expect("position");

        assert_eq!(p.curve, "XYC");
        assert_eq!(p.pair_type, crate::primitives::pool::PoolType::Volatile);
        assert_eq!(p.state, "active");
        assert_eq!(p.range.kind, "full");
        // virtual: 1 WETH @ $2000 + 2000 USDC @ $1 = $4000.
        assert_eq!(p.balances.virtual_balances.total_usd, Some(4000.0));
        assert!(!p.balances.backed, "WETH is only half pullable");
        // actual $3000 / virtual $4000; the 0.5 WETH gap surfaces as a shortfall string.
        assert_eq!(p.balances.shortfall.as_deref(), Some("short 0.5 WETH"));
        assert_eq!(p.balances.coverage, Some(0.75));
        // opening = ship-block legs only (the block-200 swap is excluded): 1 WETH + 2000 USDC.
        assert_eq!(p.balances.opening.entries.len(), 2);
        assert_eq!(p.balances.opening.total_usd, Some(4000.0));
        // economics: window volume 1 WETH = $2000; fee 5bps → $1.
        assert_eq!(p.economics.volume_usd, Some(2000.0));
        assert_eq!(p.economics.fees_usd, Some(1.0));
        let stats = p.active_stats.expect("detail carries active_stats");
        assert_eq!(stats.fills_7d, 5);
        assert_eq!(stats.quote_uptime_pct, Some(75.0));
    }

    #[tokio::test]
    async fn roster_counts_and_values_makers() {
        let snapshot = Snapshot::from_strategies([xyc_strategy(9), xyc_strategy(10)]);
        let svc = service(snapshot, HashMap::new());

        let roster = svc.roster().await.unwrap();
        assert_eq!(roster.len(), 2);
        assert_eq!(roster[0].active_positions, 1);
        assert_eq!(roster[0].shared_liquidity_usd, Some(4000.0));
    }

    #[tokio::test]
    async fn dashboard_kpis_fill_share_and_insight() {
        // Maker 9 backs only half its WETH; a competitor (maker 10) quotes the same pair cheaper.
        let pullable = HashMap::from([
            (addr(WETH), U256::from(500_000_000_000_000_000u128)), // 0.5 WETH
            (addr(USDC), U256::from(2_000_000_000u64)),            // 2000 USDC
        ]);
        let mut competitor = xyc_strategy(10);
        competitor.curve = CurveSpec::Priceable {
            curve: Curve::Xyc,
            fees_in_bps: vec![300_000], // 3 real bps < maker 9's 5 bps
        };
        let svc = service(
            Snapshot::from_strategies([xyc_strategy(9), competitor]),
            pullable,
        );

        let d = svc.dashboard(MakerId(addr(9))).await.unwrap();

        assert_eq!(d.active_positions, 1);
        // shared = 1 WETH ($2000) + 2000 USDC = $4000; pullable = 0.5 WETH + 2000 USDC = $3000.
        assert_eq!(d.kpis.shared_liquidity_usd, Some(4000.0));
        assert_eq!(d.kpis.pullable_usd, Some(3000.0));
        assert_eq!(d.kpis.wallet_balance_usd, Some(3000.0));
        assert_eq!(d.kpis.shared_liq_ratio, Some(0.75));
        assert_eq!(d.kpis.volume_usd, Some(2000.0));
        assert_eq!(d.kpis.fees_usd, Some(1.0));
        // Current volume $2000 vs the previous window's $1000 → +100%.
        assert_eq!(d.kpis.volume_change_pct, Some(100.0));
        assert_eq!(d.kpis.fees_change_pct, Some(100.0));
        // Net trade flow: inflow $2010 − outflow $2000 = +$10 against start $3990 ≈ +0.25%.
        let liq = d.kpis.shared_liquidity_change_pct.expect("net flow priced");
        assert!((liq - 0.2506).abs() < 0.01, "liq change {liq}");
        // Fill share: 20 of 50 fills on the maker's pairs.
        assert_eq!(d.fill_share.filled, 20);
        assert_eq!(d.fill_share.pair_fills, 50);
        assert_eq!(d.fill_share.share_pct, Some(40.0));
        assert_eq!(d.latency_p50_ms, Some(120));
        // Insight names the cheaper competitor.
        let insight = d.insight.expect("competitor undercuts");
        assert!(
            insight.contains("3 bps") && insight.contains("5 bps"),
            "{insight}"
        );
    }

    #[tokio::test]
    async fn inventory_groups_positions_by_token() {
        // Maker 9 holds a WETH/USDC and a USDC/USDT position, so USDC is shared across both.
        let holdings = HashMap::from([
            (addr(USDC), U256::from(5_000_000_000u64)), // 5000 USDC wallet
            (addr(WETH), U256::from(1_000_000_000_000_000_000u128)), // 1 WETH
            (addr(USDT), U256::from(1_000_000_000u64)), // 1000 USDT
        ]);
        let snap = Snapshot::from_strategies([xyc_strategy(9), usdc_usdt_strategy(9, 8)]);
        let svc = service(snap, holdings);

        let inv = svc.inventory(MakerId(addr(9))).await.unwrap();

        // Three token rows, ordered by address: USDC(2), WETH(3), USDT(4).
        assert_eq!(inv.len(), 3);

        let usdc = &inv[0];
        assert_eq!(usdc.token.symbol, "USDC");
        assert_eq!(usdc.legs.len(), 2, "USDC is held by both positions");
        assert_eq!(usdc.shared.display, "3000"); // 2000 + 1000
        assert_eq!(usdc.shared.usd, Some(3000.0));
        assert_eq!(usdc.wallet.usd, Some(5000.0));
        assert!(usdc.legs.iter().any(|l| l.pair.contains("WETH")));
        assert!(usdc.legs.iter().any(|l| l.pair.contains("USDT")));
        // Opening comes from the event log: the WETH/USDC position opened at 2000 USDC.
        let weth_leg = usdc.legs.iter().find(|l| l.pair.contains("WETH")).unwrap();
        assert_eq!(weth_leg.opening.display, "2000");

        let weth = &inv[1];
        assert_eq!(weth.token.symbol, "WETH");
        assert_eq!(weth.legs.len(), 1);
        assert_eq!(weth.shared.usd, Some(2000.0)); // 1 WETH @ $2000
        assert_eq!(weth.wallet.usd, Some(2000.0));

        let usdt = &inv[2];
        assert_eq!(usdt.token.symbol, "USDT");
        assert_eq!(usdt.legs.len(), 1);
        assert_eq!(usdt.shared.usd, Some(1000.0));
    }
}
