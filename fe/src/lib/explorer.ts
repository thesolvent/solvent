import type {
  ActivityRecord,
  ExplorerStats,
  TokenQuantity,
  TradeRecord,
} from "@/data/explorer";

const STATUS_TONE: Record<string, { background: string; color: string }> = {
  confirmed: {
    background: "var(--lime-wash-soft)",
    color: "var(--green-darkest)",
  },
  declined: { background: "var(--warn-bg)", color: "var(--warn-ink)" },
  failed: { background: "var(--err-bg)", color: "var(--err-ink)" },
  pending: { background: "#eef0f4", color: "#4a5a72" },
};

export function shortHash(value: string | null): string {
  return value ? `${value.slice(0, 8)}…${value.slice(-6)}` : "—";
}

export function tokenText(quantity: TokenQuantity): string {
  const amount = Number(quantity.display).toLocaleString("en-US", {
    maximumSignificantDigits: 12,
  });
  return `${amount} ${quantity.symbol}`;
}

function timestamp(at: number | null): string {
  return at == null
    ? "—"
    : new Date(at * 1000).toLocaleString(undefined, {
        dateStyle: "medium",
        timeStyle: "short",
      });
}

export function relativeTime(at: number | null, now = Date.now()): string {
  if (at == null) return "—";
  const seconds = Math.max(0, Math.floor(now / 1000) - at);
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
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

const numberText = (value: number | null | undefined) =>
  value == null ? "—" : value.toLocaleString("en-US");
const percent = (value: number | null | undefined) =>
  value == null ? "—" : `${value.toFixed(2)}%`;

export function explorerStats(stats: ExplorerStats | undefined) {
  return [
    {
      label: "Block height",
      term: "blockHeight" as const,
      value: numberText(stats?.blockHeight),
      sub: "live",
      accent: "var(--green)",
    },
    {
      label: "Events, 24h",
      term: "events24h" as const,
      value: numberText(stats?.events24h),
      sub: "",
      accent: "var(--green-deep)",
    },
    {
      label: "Trades settled",
      term: "tradesSettled" as const,
      value: numberText(stats?.tradesSettled),
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
      value: numberText(stats?.activeMakers),
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
        : `blk ${numberText(trade.blockNumber)}`,
    input: tokenText(trade.input),
    output: `${trade.status === "confirmed" ? "" : "min. "}${tokenText(trade.output)}`,
    makers: numberText(trade.makers),
    impact: percent(trade.priceImpactPct),
    status: trade.status,
    transactionLabel: shortHash(trade.txHash),
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
    who: shortHash(record.maker),
    tx: shortHash(record.txHash),
    when: `${record.blockNumber == null ? "block unknown" : `blk ${numberText(record.blockNumber)}`} · ${relativeTime(record.at)}`,
    flow: record.amount
      ? tokenText(record.amount)
      : record.kind === "docked"
        ? "Position closed"
        : `Strategy ${shortHash(record.strategyHash)}`,
    text: `strategy ${shortHash(record.strategyHash)}`,
  };
}

export function tradeDetail(trade: TradeRecord) {
  const row = tradeRow(trade);
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
      curve: leg.curve ?? "—",
      maker: leg.maker,
      hash: leg.strategyHash,
      chainId: leg.chainId,
      name: shortHash(leg.maker),
      shortHash: shortHash(leg.strategyHash),
      tag: leg.maker.slice(2, 4).toUpperCase(),
      amt: `${tokenText(leg.input)} → ${tokenText(leg.output)}`,
      input: leg.input,
      output: leg.output,
      share: `${leg.sharePct.toFixed(1)}%`,
      barW: `${leg.sharePct}%`,
    })),
    facts: [
      { label: "Taker", value: shortHash(trade.taker), fullValue: trade.taker },
      {
        label: "Order hash",
        value: shortHash(trade.orderHash),
        fullValue: trade.orderHash ?? "—",
      },
      { label: "Deadline", value: timestamp(trade.deadlineAt) },
      { label: "Signature", value: trade.signaturePresent ? "Provided" : "—" },
    ],
    profit: trade.surplus ? tokenText(trade.surplus) : "—",
    profitTag: ["declined", "failed"].includes(trade.status)
      ? `not earned — ${trade.status}`
      : "route estimate · net of estimated gas",
    empty: trade.legs.length === 0,
    emptyText: "No maker legs recorded for this order.",
  };
}
