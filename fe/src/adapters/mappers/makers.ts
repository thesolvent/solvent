import type {
  InventoryRow as ApiInventory,
  MakerDashboard as ApiDashboard,
  MakerSummary as ApiSummary,
  MakerTrade,
  Position as ApiPosition,
} from "@solvent/sdk/client";
import type {
  InventoryAsset,
  MakerDashboard,
  MakerSettlement,
  MakerSummary,
  Position,
  PositionBalance,
  ValuedAmount,
} from "@/data/makers";
import { toTrade } from "./explorer";

export function toMaker(api: ApiSummary): MakerSummary {
  return {
    address: api.maker,
    activePositions: api.active_positions,
    sharedLiquidityUsd: api.shared_liquidity_usd ?? null,
  };
}

export function toDashboard(api: ApiDashboard): MakerDashboard {
  const k = api.kpis;
  return {
    address: api.maker,
    activePositions: api.active_positions,
    windowDays: api.window_days,
    sharedLiquidityUsd: k.shared_liquidity_usd ?? null,
    volumeUsd: k.volume_usd ?? null,
    feesUsd: k.fees_usd ?? null,
    walletUsd: k.wallet_balance_usd ?? null,
    pullableUsd: k.pullable_usd ?? null,
    coverage: k.shared_liq_ratio ?? null,
    liquidityChangePct: k.shared_liquidity_change_pct ?? null,
    volumeChangePct: k.volume_change_pct ?? null,
    feesChangePct: k.fees_change_pct ?? null,
    fills: api.fill_share.filled,
    pairFills: api.fill_share.pair_fills,
    sharePct: api.fill_share.share_pct ?? null,
    latencyMs: api.latency_p50_ms ?? null,
    previousLatencyMs: api.previous_latency_p50_ms ?? null,
    fillsChangePct: api.fills_change_pct ?? null,
    activity: api.activity.map((bucket) => ({
      from: bucket.from,
      to: bucket.to,
      fills: bucket.fills,
      latencyMs: bucket.latency_p50_ms ?? null,
    })),
    insight: api.insight ?? null,
  };
}

function amount(value: { display: string; usd?: number | null }): ValuedAmount {
  return { display: value.display, usd: value.usd ?? null };
}

function balances(
  entries: ApiPosition["balances"]["actual"]["entries"],
): PositionBalance[] {
  return entries.map(({ token, amount: value }) => ({
    address: token.address,
    symbol: token.symbol,
    ...amount(value),
  }));
}

export function toPosition(api: ApiPosition): Position {
  return {
    hash: api.strategy_hash,
    maker: api.maker,
    pair: api.pair,
    quoteSymbol: api.quote.symbol,
    curve: api.curve,
    state: api.state,
    feeBps: api.fee_bps,
    range: api.range.label,
    rangeKind: api.range.kind,
    lowerPrice:
      api.range.lower_price == null ? null : Number(api.range.lower_price),
    upperPrice:
      api.range.upper_price == null ? null : Number(api.range.upper_price),
    pairType: api.pair_type,
    volumeUsd: api.economics.volume_usd ?? null,
    spot: api.mid_price ?? null,
    committed: balances(api.balances.virtual.entries),
    actual: balances(api.balances.actual.entries),
    opening: balances(api.balances.opening.entries),
    committedUsd: api.balances.virtual.total_usd ?? null,
    actualUsd: api.balances.actual.total_usd ?? null,
    coverage: api.balances.coverage ?? null,
    backed: api.balances.backed,
    shortfall: api.balances.shortfall ?? null,
    feesUsd: api.economics.fees_usd ?? null,
    apyPct: api.economics.apy_pct ?? null,
    fills7d: api.active_stats?.fills_7d ?? null,
    dailyFills: api.active_stats?.daily_fills ?? [],
    volume7dUsd: api.active_stats?.volume_7d_usd ?? null,
    lastFillAt: api.active_stats?.last_fill_at ?? null,
    quoteUptimePct: api.active_stats?.quote_uptime_pct ?? null,
  };
}

export function toInventory(api: ApiInventory): InventoryAsset {
  return {
    address: api.token.address,
    symbol: api.token.symbol,
    wallet: amount(api.wallet),
    shared: amount(api.shared),
    feesUsd: api.fees_usd ?? null,
    apyPct: api.apy_pct ?? null,
    legs: api.legs.map((leg) => ({
      hash: leg.strategy_hash,
      pair: leg.pair,
      curve: leg.curve,
      feeBps: leg.fee_bps,
      current: amount(leg.current),
      opening: amount(leg.opening),
      feesUsd: leg.fees_usd ?? null,
      apyPct: leg.apy_pct ?? null,
      coverage: leg.coverage ?? null,
    })),
  };
}

export function toMakerSettlement(api: MakerTrade): MakerSettlement {
  return {
    ...toTrade(api),
    feeUsd: api.fee_usd ?? null,
    sharePct: api.share_pct ?? null,
  };
}
