import type { ActivityEvent, Stats, Trade } from "@solvent/sdk/client";
import type {
  ActivityRecord,
  ExplorerStats,
  TradeRecord,
} from "@/data/explorer";

export function toTrade(api: Trade): TradeRecord {
  const legs = api.legs ?? [];
  const total = legs.reduce((sum, leg) => sum + BigInt(leg.amount_out.raw), 0n);
  return {
    signaturePresent: api.signature_present ?? null,
    id: api.id,
    status: api.status,
    taker: api.taker,
    input: {
      symbol: api.input.token.symbol,
      display: api.input.amount.display,
    },
    output: {
      symbol: api.output.token.symbol,
      display: api.output.amount.display,
    },
    surplus: api.surplus
      ? { symbol: api.input.token.symbol, display: api.surplus.display }
      : null,
    priceImpactPct: api.price_impact_pct ?? null,
    makers:
      api.legs == null
        ? null
        : new Set(legs.map((leg) => leg.maker.toLowerCase())).size,
    txHash: api.tx_hash ?? null,
    blockNumber: api.block_number ?? null,
    createdAt: api.created_at,
    settledAt: api.settled_at ?? null,
    deadlineAt: api.deadline_block ?? null,
    orderHash: api.order_hash ?? null,
    lifecycle: api.lifecycle ?? [],
    legs: legs.map((leg) => ({
      maker: leg.maker,
      strategyHash: leg.strategy_hash,
      curve: leg.curve ?? null,
      input: { symbol: api.input.token.symbol, display: leg.amount_in.display },
      output: {
        symbol: api.output.token.symbol,
        display: leg.amount_out.display,
      },
      sharePct:
        total === 0n
          ? 0
          : Number((BigInt(leg.amount_out.raw) * 10_000n) / total) / 100,
    })),
  };
}

export function toActivity(api: ActivityEvent): ActivityRecord {
  return {
    id: [
      api.tx_hash,
      api.log_index,
      api.block_number,
      api.kind,
      api.maker,
      api.strategy_hash,
      api.at,
    ].join(":"),
    kind: api.kind,
    maker: api.maker,
    strategyHash: api.strategy_hash,
    amount: api.token
      ? { symbol: api.token.token.symbol, display: api.token.amount.display }
      : null,
    at: api.at,
    blockNumber: api.block_number ?? null,
    txHash: api.tx_hash ?? null,
  };
}

export function toStats(api: Stats): ExplorerStats {
  return {
    blockHeight: api.block_height,
    events24h: api.events_24h ?? null,
    tradesSettled: api.trades_settled ?? null,
    confirmedPct: api.confirmed_pct ?? null,
    medianImpactPct: api.median_impact_pct ?? null,
    activeMakers: api.active_makers ?? null,
    quotingNow: api.quoting_now ?? null,
  };
}
