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
use crate::primitives::amount::{Amount, TokenAmount, TokenAmounts};
use crate::primitives::maker::{
    ActiveStats, Economics, FillShare, InventoryLeg, InventoryRow, MakerDashboard, MakerKpis,
    MakerSummary, Position, PositionBalances, PriceRange, Split,
};
use crate::primitives::pool::classify_pair;
use crate::primitives::registry::{
    curve_label, fee_in_bps, price_range, AquaEvent, Curve, CurveKind, CurveSpec, MakerStrategy,
    PositionRange, RangeKind, TokenPair,
};
use crate::primitives::{ChainId, MakerId, StrategyHash, Usd};
use crate::registry::SharedSnapshot;
use crate::valuation::Valuation;
use crate::SolventError;

const DAY: u64 = 86_400;
/// The activity window for economics and active-stats.
const WINDOW_DAYS: u64 = 7;

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
        for (maker, (holdings_count, holdings)) in grouped {
            let priced: Vec<(crate::primitives::asset::Token, U256)> = holdings
                .into_iter()
                .map(|(addr, amt)| (self.assets.token_or_default(addr), amt))
                .collect();
            roster.push(MakerSummary {
                maker: maker.0,
                active_positions: holdings_count,
                shared_liquidity_usd: self.valuation.tvl(&priced).await.map(Usd::to_f64),
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

        // Precise headline totals reuse the position builder, so the dashboard can never drift from
        // the positions list.
        let positions = self.positions(maker).await?;
        let active_positions = positions.len() as u64;
        let shared_liquidity_usd = sum_usd(
            positions
                .iter()
                .map(|p| p.balances.virtual_balances.total_usd),
        );
        let pullable_usd = sum_usd(positions.iter().map(|p| p.balances.actual.total_usd));
        let volume_usd = sum_usd(positions.iter().map(|p| p.economics.volume_usd));
        let fees_usd = sum_usd(positions.iter().map(|p| p.economics.fees_usd));

        // Wallet balance (raw, uncapped) across every token the maker commits.
        let snapshot = self.registry.load();
        let strategies: Vec<MakerStrategy> = snapshot
            .strategies_for_maker(maker)
            .filter(|s| s.active && matches!(s.curve, CurveSpec::Priceable { .. }))
            .cloned()
            .collect();
        let tokens: Vec<Address> = strategies
            .iter()
            .flat_map(|s| s.balances.keys().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let wallet_balance_usd = self.wallet_usd(maker.0, &tokens).await?;
        let shared_liq_ratio = ratio(pullable_usd, shared_liquidity_usd);

        // Trade-derived deltas need the previous equal window.
        let cur = self
            .metrics
            .maker(maker, now.saturating_sub(window), now)
            .await?;
        let prev = self
            .metrics
            .maker(
                maker,
                now.saturating_sub(2 * window),
                now.saturating_sub(window),
            )
            .await?;
        let volume_change_pct = change_pct(volume_usd, self.usd_of(&prev.volume).await);
        // Fees track volume at a stable fee mix, so the fee delta mirrors the volume delta.
        let fees_change_pct = volume_change_pct;
        let shared_liquidity_change_pct = self.liquidity_change(&cur, shared_liquidity_usd).await;

        // Fill share: the maker's fills over all fills on the pairs it quotes.
        let pairs: Vec<TokenPair> = strategies
            .iter()
            .filter_map(|s| s.pair())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let pair_fills = self
            .metrics
            .pair_fills(&pairs, now.saturating_sub(window))
            .await?;

        Ok(MakerDashboard {
            maker: maker.0,
            window_days: WINDOW_DAYS as u32,
            active_positions,
            kpis: MakerKpis {
                shared_liquidity_usd,
                volume_usd,
                wallet_balance_usd,
                pullable_usd,
                shared_liq_ratio,
                fees_usd,
                shared_liquidity_change_pct,
                volume_change_pct,
                fees_change_pct,
            },
            fill_share: FillShare {
                filled: cur.fills,
                pair_fills,
                share_pct: pct_of(cur.fills, pair_fills),
            },
            latency_p50_ms: cur.latency_p50_ms,
            insight: self.undercut_insight(maker, &strategies),
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
        Ok(self.valuation.tvl(&priced).await.map(Usd::to_f64))
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
        let mut best: Option<(u32, TokenPair, Address, u32, u32)> = None;
        for mine in strategies {
            let CurveSpec::Priceable { fees_in_bps, .. } = &mine.curve else {
                continue;
            };
            let (Some(pair), my_fee) = (mine.pair(), fee_in_bps(fees_in_bps)) else {
                continue;
            };
            for other in snapshot.active_strategies() {
                if other.key.maker == maker || other.pair() != Some(pair) {
                    continue;
                }
                let CurveSpec::Priceable { fees_in_bps, .. } = &other.curve else {
                    continue;
                };
                let their_fee = fee_in_bps(fees_in_bps);
                let gap = my_fee.saturating_sub(their_fee);
                if gap > 0 && best.is_none_or(|(g, ..)| gap > g) {
                    best = Some((gap, pair, other.key.maker.0, their_fee, my_fee));
                }
            }
        }
        best.map(|(_, pair, competitor, their_fee, my_fee)| {
            format!(
                "{} quotes {} at {} bps vs your {} bps",
                short_addr(competitor),
                self.assets.pair_label(&pair),
                their_fee,
                my_fee
            )
        })
    }

    /// The maker's positions re-grouped by token — the Assets tab. Each token row aggregates the
    /// positions holding it (its `legs`), with wallet vs committed balances and economics.
    pub async fn inventory(&self, maker: MakerId) -> Result<Vec<InventoryRow>, SolventError> {
        // Detail positions carry per-token opening and economics; transpose them into per-token rows.
        let strategies: Vec<MakerStrategy> = self
            .registry
            .load()
            .strategies_for_maker(maker)
            .cloned()
            .collect();
        let mut grouped: BTreeMap<Address, (crate::primitives::asset::Token, Vec<InventoryLeg>)> =
            BTreeMap::new();
        for strategy in &strategies {
            let Some(position) = self.build(strategy, true).await? else {
                continue;
            };
            for entry in &position.balances.virtual_balances.entries {
                let addr = entry.token.address;
                let opening = position
                    .balances
                    .opening
                    .entries
                    .iter()
                    .find(|e| e.token.address == addr)
                    .map(|e| e.amount.clone())
                    .unwrap_or_else(|| Amount::from_base_units(U256::ZERO, entry.token.decimals));
                grouped
                    .entry(addr)
                    .or_insert_with(|| (entry.token.clone(), Vec::new()))
                    .1
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

        let tokens: Vec<Address> = grouped.keys().copied().collect();
        let holdings = self.balances.holdings(maker.0, &tokens).await?;
        let mut rows = Vec::with_capacity(grouped.len());
        for (addr, (token, legs)) in grouped {
            let wallet_raw = holdings.get(&addr).map_or(U256::ZERO, |h| h.balance);
            let wallet = self
                .valuation
                .amount(wallet_raw, addr, token.decimals)
                .await;
            let shared_raw = legs.iter().fold(U256::ZERO, |acc, leg| {
                acc.saturating_add(leg.current.raw.parse::<U256>().unwrap_or(U256::ZERO))
            });
            let shared = self
                .valuation
                .amount(shared_raw, addr, token.decimals)
                .await;
            let fees_usd = sum_usd(legs.iter().map(|leg| leg.fees_usd));
            let apy_pct = annualized_apy(fees_usd, shared.usd);
            rows.push(InventoryRow {
                token,
                wallet,
                shared,
                fees_usd,
                apy_pct,
                legs,
            });
        }
        Ok(rows)
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
        let mut entries = Vec::with_capacity(balances.len());
        let mut priced = Vec::with_capacity(balances.len());
        for (addr, amount) in balances {
            let token = self.assets.token_or_default(*addr);
            entries.push(TokenAmount {
                amount: self
                    .valuation
                    .amount(*amount, token.address, token.decimals)
                    .await,
                token: token.clone(),
            });
            priced.push((token, *amount));
        }
        TokenAmounts {
            entries,
            total_usd: self.valuation.tvl(&priced).await.map(Usd::to_f64),
        }
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
        self.valuation.tvl(&priced).await.map(Usd::to_f64)
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

/// USD economics from window volume, the strategy's fee, and committed liquidity. APY annualizes the
/// window's fees over the committed liquidity: `fees/liquidity · 365/window`.
fn economics(volume_usd: Option<f64>, fee_bps: u32, liquidity_usd: Option<f64>) -> Economics {
    let fees_usd = volume_usd.map(|v| v * f64::from(fee_bps) / 10_000.0);
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
/// either side is unpriced.
fn coverage(actual: &TokenAmounts, virtual_balances: &TokenAmounts) -> Option<f64> {
    match (actual.total_usd, virtual_balances.total_usd) {
        (Some(a), Some(v)) if v > 0.0 => Some(a / v),
        _ => None,
    }
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
    use crate::deps::maker_metrics::{MakerMetrics, MakerMetricsError, TokenVolume};
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
            Arc::new(FakeOracle(pullable)),
            Arc::new(FakeEvents),
            Arc::new(FixedClock),
            ChainId(1),
        )
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
