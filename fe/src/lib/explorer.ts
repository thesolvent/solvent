import type {
  ActivityRecord,
  ExplorerStats,
  TokenQuantity,
  TradeRecord,
} from "@/data/explorer";
import { GLOSSARY, STATUS_TERMS } from "@/lib/glossary";
import { isHaltedTrade, isTerminalTrade } from "@/lib/trade-lifecycle";
import {
  DASH,
  LOCALE,
  blockNumber,
  count,
  percent,
  tokenWithSymbol,
  truncateAddress,
  truncateHash,
} from "@/lib/format";

const STATUS_TONE: Record<string, { background: string; color: string }> = {
  confirmed: {
    background: "var(--lime-wash-soft)",
    color: "var(--green-darkest)",
  },
  declined: { background: "var(--warn-bg)", color: "var(--warn-ink)" },
  failed: { background: "var(--err-bg)", color: "var(--err-ink)" },
  pending: { background: "#eef0f4", color: "#4a5a72" },
};

export function tokenText(quantity: TokenQuantity): string {
  return tokenWithSymbol(quantity.display, quantity.symbol);
}

function timestamp(at: number | null): string {
  return at == null
    ? DASH
    : new Date(at * 1000).toLocaleString(LOCALE, {
        dateStyle: "medium",
        timeStyle: "short",
      });
}

/** A gap in seconds, at the coarsest unit that still carries the decision. */
function span(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`;
  return `${Math.floor(seconds / 86400)}d`;
}

export function relativeTime(at: number | null, now = Date.now()): string {
  if (at == null) return DASH;
  return `${span(Math.max(0, Math.floor(now / 1000) - at))} ago`;
}

/**
 * A deadline as a distance from now.
 *
 * While an order can still be filled the only fact that changes a decision is how long is left;
 * the wall-clock time it was signed for is kept on the row's title for anyone reconciling logs.
 */
export function countdown(at: number | null, now = Date.now()): string {
  if (at == null) return DASH;
  const seconds = at - Math.floor(now / 1000);
  return seconds > 0 ? `in ${span(seconds)}` : `expired ${span(-seconds)} ago`;
}

export function explorerUrl(
  base: string | undefined,
  kind: "tx" | "address",
  value: string | null,
): string | undefined {
  if (!base || !value) return undefined;
  try {
    const url = new URL(
      `${base.replace(/\/$/, "")}/${kind}/${encodeURIComponent(value)}`,
    );
    return ["http:", "https:"].includes(url.protocol) ? url.href : undefined;
  } catch {
    return undefined;
  }
}

export function explorerStats(stats: ExplorerStats | undefined) {
  return [
    {
      label: "Block height",
      term: "blockHeight" as const,
      value: blockNumber(stats?.blockHeight),
      sub: "",
      accent: "var(--text-muted)",
    },
    {
      label: "Events, 24h",
      term: "events24h" as const,
      value: count(stats?.events24h),
      sub: "",
      accent: "var(--green-deep)",
    },
    {
      label: "Trades settled",
      term: "tradesSettled" as const,
      value: count(stats?.tradesSettled),
      sub:
        stats?.confirmedPct == null
          ? ""
          : `${percent(stats.confirmedPct)} confirmed`,
      accent: "var(--text-muted)",
    },
    {
      label: "Median impact",
      term: "medianImpact" as const,
      value: percent(stats?.medianImpactPct),
      sub: "",
      accent: "var(--text-muted)",
    },
    {
      label: "Active makers",
      term: "activeMakers" as const,
      value: count(stats?.activeMakers),
      sub:
        stats?.quotingNow == null
          ? ""
          : `${stats.quotingNow} strategies quoting`,
      accent: "var(--text-muted)",
    },
  ].map((stat, i) => ({
    ...stat,
    sep: i === 0 ? "transparent" : "var(--line)",
  }));
}

export function tradeRow(trade: TradeRecord) {
  return {
    id: trade.id,
    pair: `${trade.input.symbol}/${trade.output.symbol}`,
    blockLabel:
      trade.blockNumber == null
        ? "not settled"
        : `blk ${blockNumber(trade.blockNumber)}`,
    input: tokenText(trade.input),
    output: `${trade.status === "confirmed" ? "" : "min. "}${tokenText(trade.output)}`,
    makers: count(trade.makers),
    impact: percent(trade.priceImpactPct),
    status: trade.status,
    transactionLabel: truncateHash(trade.txHash),
    statusStyle: STATUS_TONE[trade.status] ?? STATUS_TONE.pending,
  };
}

const KINDS: Record<
  string,
  { label: string; background: string; color: string }
> = {
  pulled: {
    label: "pull",
    background: "var(--lime-wash-soft)",
    color: "var(--green-darkest)",
  },
  pushed: {
    label: "push",
    background: "var(--info-bg)",
    color: "var(--info-ink)",
  },
  docked: {
    label: "dock",
    background: "var(--surface)",
    color: "var(--text-mid)",
  },
  shipped: { label: "register", background: "#f4f0e2", color: "#7a6320" },
};

export function activityRow(record: ActivityRecord) {
  const kind = KINDS[record.kind] ?? { ...KINDS.docked, label: record.kind };
  return {
    ...record,
    kind: kind.label,
    kindBg: kind.background,
    kindFg: kind.color,
    who: truncateAddress(record.maker),
    tx: truncateHash(record.txHash),
    when: `${record.blockNumber == null ? "block unknown" : `blk ${blockNumber(record.blockNumber)}`} · ${relativeTime(record.at)}`,
    flow: record.amount
      ? tokenText(record.amount)
      : record.kind === "docked"
        ? "Position closed"
        : `Strategy ${truncateHash(record.strategyHash)}`,
    text: `strategy ${truncateHash(record.strategyHash)}`,
  };
}

export function tradeDetail(trade: TradeRecord, now = Date.now()) {
  const row = tradeRow(trade);
  const halted = isHaltedTrade(trade.status);
  const statusTerm = STATUS_TERMS[trade.status];
  return {
    title: `Trade #${trade.id}`,
    status: trade.status,
    statusStyle: row.statusStyle,
    blockLabel: `${row.blockLabel} · ${relativeTime(trade.settledAt ?? trade.createdAt)}`,
    transactionLabel: row.transactionLabel,
    summary: [
      {
        label: "In → out",
        value: `${row.input} → ${row.output}`,
      },
      { label: "Price impact", value: row.impact },
      { label: "Resolver", value: "Zero-inventory" },
      { label: "Makers", value: row.makers },
    ].map((stat, i) => ({
      ...stat,
      sep: i === 0 ? "transparent" : "var(--line)",
    })),
    legs: trade.legs.map((leg) => ({
      curve: leg.curve ?? DASH,
      maker: leg.maker,
      hash: leg.strategyHash,
      chainId: leg.chainId,
      name: truncateAddress(leg.maker),
      shortHash: truncateHash(leg.strategyHash),
      tag: leg.maker.slice(2, 4).toUpperCase(),
      amt: `${tokenText(leg.input)} → ${tokenText(leg.output)}`,
      input: leg.input,
      output: leg.output,
      share: percent(leg.sharePct, { digits: 1 }),
      barW: `${leg.sharePct}%`,
    })),
    facts: [
      {
        label: "Taker",
        value: truncateAddress(trade.taker),
        fullValue: trade.taker,
      },
      {
        label: "Order hash",
        value: truncateHash(trade.orderHash),
        fullValue: trade.orderHash ?? DASH,
      },
      {
        label: "Deadline",
        // Once the trade is terminal the countdown is history; the signed time is the useful fact.
        value: isTerminalTrade(trade.status)
          ? timestamp(trade.deadlineAt)
          : countdown(trade.deadlineAt, now),
        fullValue: timestamp(trade.deadlineAt),
      },
      {
        label: "Signature",
        value: trade.signaturePresent ? "Provided" : DASH,
      },
    ],
    profitLabel: halted ? "Not earned" : "Expected profit",
    profit: halted || !trade.surplus ? DASH : tokenText(trade.surplus),
    profitTag:
      halted && statusTerm
        ? GLOSSARY[statusTerm]
        : "route estimate · net of estimated gas",
    empty: trade.legs.length === 0,
    emptyText: "No maker legs recorded for this order.",
  };
}
