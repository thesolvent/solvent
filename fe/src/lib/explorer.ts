import { SolventApiError } from "@solvent/sdk/client";
import type {
  ActivityRecord,
  ExplorerStats,
  TokenQuantity,
  TradeRecord,
} from "@/data/explorer";

export const STATUS_TONE: Record<string, [string, string]> = {
  confirmed: ["var(--lime-wash-soft)", "var(--green-darkest)"],
  declined: ["var(--warn-bg)", "var(--warn-ink)"],
  "reorg-open": ["var(--info-bg)", "var(--info-ink)"],
  failed: ["var(--err-bg)", "var(--err-ink)"],
  pending: ["#eef0f4", "#4a5a72"],
};

export const DROP_OPTIONS = {
  xpType: ["All types", "pull", "push", "dock", "register"],
  xpEnt: ["All entities", "Maker", "Resolver"],
  xpStatus: [
    "All status",
    "created",
    "quoted",
    "reserved",
    "simulated",
    "submitted",
    "confirmed",
    "declined",
    "failed",
  ],
  xpPair: ["All pairs"],
};
export type DropKey = keyof typeof DROP_OPTIONS;

export function isTerminalTrade(status: string): boolean {
  return ["confirmed", "declined", "failed"].includes(status);
}

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

export function isMissingTrade(error: unknown): boolean {
  return error instanceof SolventApiError && [400, 404].includes(error.status);
}

export function tradeProblem(error: unknown): string {
  if (isMissingTrade(error)) return "Trade not found.";
  return "Couldn’t load this trade. Try again.";
}

const numberText = (value: number | null | undefined) =>
  value == null ? "—" : value.toLocaleString("en-US");
const percent = (value: number | null | undefined) =>
  value == null ? "—" : `${value.toFixed(2)}%`;

export function explorerStats(stats: ExplorerStats | undefined) {
  return [
    {
      label: "Block height",
      value: numberText(stats?.blockHeight),
      sub: "",
      accent: "var(--green)",
    },
    {
      label: "Events, 24h",
      value: numberText(stats?.events24h),
      sub: "",
      accent: "var(--green-deep)",
    },
    {
      label: "Trades settled",
      value: numberText(stats?.tradesSettled),
      sub:
        stats?.confirmedPct == null
          ? ""
          : `${percent(stats.confirmedPct)} confirmed`,
      accent: "var(--text-muted)",
    },
    {
      label: "Median impact",
      value: percent(stats?.medianImpactPct),
      sub: "",
      accent: "var(--text-muted)",
    },
    {
      label: "Active makers",
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
  const [stBg, stFg] = STATUS_TONE[trade.status] ?? STATUS_TONE.pending;
  return {
    id: trade.id,
    pair: `${trade.input.symbol}/${trade.output.symbol}`,
    blk:
      trade.blockNumber == null
        ? "not settled"
        : `blk ${numberText(trade.blockNumber)}`,
    inn: tokenText(trade.input),
    out: `${trade.status === "confirmed" ? "" : "min. "}${tokenText(trade.output)}`,
    makers: numberText(trade.makers),
    impact: percent(trade.priceImpactPct),
    status: trade.status,
    tx: shortHash(trade.txHash),
    stBg,
    stFg,
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
    when: `${record.blockNumber == null ? "block unknown" : `blk ${numberText(record.blockNumber)}`} · ${timestamp(record.at)}`,
    flow: record.amount
      ? tokenText(record.amount)
      : record.kind === "docked"
        ? "Position closed"
        : `Strategy ${shortHash(record.strategyHash)}`,
    text: `strategy ${shortHash(record.strategyHash)}`,
  };
}

const STAGES = [
  "Created",
  "Quoted",
  "Reserved",
  "Simulated",
  "Submitted",
  "Confirmed",
];

export function tradeDetail(trade: TradeRecord, hoveredStage: number | null) {
  const terminal = isTerminalTrade(trade.status);
  const recorded = new Map(
    trade.lifecycle.map((stage) => [stage.status, stage.at]),
  );
  const stageDone = STAGES.filter((name) =>
    recorded.has(name.toLowerCase()),
  ).length;
  const latestStage = STAGES.reduce(
    (latest, name, i) =>
      recorded.has(name.toLowerCase()) || name.toLowerCase() === trade.status
        ? i
        : latest,
    -1,
  );
  const [stBg, stFg] = STATUS_TONE[trade.status] ?? STATUS_TONE.pending;
  const row = tradeRow(trade);
  const elapsed =
    trade.settledAt == null
      ? null
      : Math.max(0, trade.settledAt - trade.createdAt);
  return {
    id: `Trade #${trade.id}`,
    status: trade.status,
    stBg,
    stFg,
    meta: row.blk,
    tx: row.tx,
    summary: [
      {
        label:
          trade.status === "confirmed"
            ? "Input → received"
            : "Input → minimum output",
        value: `${tokenText(trade.input)} → ${tokenText(trade.output)}`,
      },
      { label: "Price impact", value: row.impact },
      { label: "Resolver", value: "Zero-inventory" },
      { label: "Makers", value: row.makers },
    ].map((stat, i) => ({
      ...stat,
      sep: i === 0 ? "transparent" : "var(--line)",
    })),
    stageDone,
    headMeta: elapsed == null ? "pending" : `${elapsed}s`,
    phases: [
      {
        tag: "Phase 1",
        name: "Quote & reserve",
        rule: recorded.has("reserved") ? "var(--lime)" : "var(--line)",
      },
      {
        tag: "Phase 2",
        name: "Simulate & settle",
        rule: recorded.has("confirmed") ? "var(--ink)" : "var(--line)",
      },
    ],
    steps: STAGES.map((label, i) => {
      const at = recorded.get(label.toLowerCase());
      const done = at !== undefined;
      const current = !terminal && !done && i === latestStage + 1;
      const hot = hoveredStage === i;
      const missing =
        trade.status === "confirmed" || i <= latestStage
          ? "not recorded"
          : terminal
            ? "not reached"
            : current
              ? "awaiting"
              : "pending";
      return {
        label,
        done,
        current,
        barX: `calc(${(i * 16.666).toFixed(3)}% + 3px)`,
        barStyle: done ? "solid" : "dashed",
        barBd: done ? (i < 3 ? "var(--lime)" : "var(--ink)") : "#e0e0dc",
        barBg: done ? (i < 3 ? "var(--lime)" : "var(--ink)") : "transparent",
        leadH: `${i % 2 ? 26 : 8}px`,
        state: done ? "" : missing,
        meta: done ? `+${Math.max(0, at - trade.createdAt)}s` : missing,
        scale: hot ? "translateY(-2px)" : "none",
        barShadow: hot && done ? "0 6px 16px rgba(11,11,11,0.16)" : "none",
        delay: done ? `${i * 130}ms` : "0ms",
        fg: done ? "var(--ink)" : "var(--text-dim)",
      };
    }),
    legs: trade.legs.map((leg) => ({
      maker: leg.maker,
      hash: leg.strategyHash,
      name: shortHash(leg.maker),
      shortHash: shortHash(leg.strategyHash),
      tag: leg.maker.slice(2, 4).toUpperCase(),
      amt: `${tokenText(leg.input)} → ${tokenText(leg.output)}`,
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
      { label: "Created", value: timestamp(trade.createdAt) },
    ],
    profit: trade.surplus ? tokenText(trade.surplus) : "—",
    profitTag: ["declined", "failed"].includes(trade.status)
      ? `not earned — ${trade.status}`
      : "route estimate · net of estimated gas",
    empty: trade.legs.length === 0,
    emptyText: "No maker legs recorded for this order.",
  };
}
