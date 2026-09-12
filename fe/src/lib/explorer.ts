import { formatUnits } from "viem";
import type {
  ActivityRecord,
  ExplorerStats,
  TokenQuantity,
  TradeRecord,
  ObservedOrder,
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

/** A raw base-unit integer as a human amount. The order log stores base units; the catalog knows
 *  the decimals, so the caller passes them in — printing 105064501461243592704 beside "WETH" is
 *  not a smaller error than printing the wrong number. */
function amountText(raw: string, decimals: number): string {
  const scaled = Number(formatUnits(BigInt(raw), decimals));
  if (!Number.isFinite(scaled)) return raw;
  return scaled.toLocaleString(undefined, { maximumFractionDigits: 6 });
}

export function tokenText(quantity: TokenQuantity): string {
  const amount = Number(quantity.display).toLocaleString("en-US", {
    maximumSignificantDigits: 12,
  });
  return `${amount} ${quantity.symbol}`;
}

/** The venue an order came from, as a reader would name it. */
function sourceLabel(source: string): string {
  if (source === "uniswapx") return "UniswapX";
  if (source === "solvent") return "Solvent API";
  if (source === "oneinch") return "1inch";
  return source;
}

const SOURCE_KEYS: Record<string, string> = {
  UniswapX: "uniswapx",
  "1inch": "oneinch",
  "Solvent API": "solvent",
};

/** The wire value the `/v1/orders` source filter expects, from the label the source dropdown
 *  shows — the inverse of `sourceLabel`. */
export function sourceKey(label: string): string | undefined {
  return SOURCE_KEYS[label];
}

const STATE_KEYS: Record<string, string> = {
  Filled: "filled",
  Declined: "declined",
  "Failed on-chain": "failed_onchain",
  Unprofitable: "unprofitable",
  Refused: "refused",
  "Pricing…": "pricing",
};

/** The wire value the `/v1/orders` state filter expects, from the label the state dropdown shows
 *  — the backend buckets verdict plus trade lifecycle the same way `orderState` does below. */
export function stateKey(label: string): string | undefined {
  return STATE_KEYS[label];
}

/** The single letter a venue's badge shows — a mark of our own, not a reproduction of the
 *  protocol's real logo artwork. */
function sourceGlyph(source: string): string {
  if (source === "uniswapx") return "X";
  if (source === "oneinch") return "1";
  return "S";
}

/** What a reader sees on hover: the order type this venue actually sent, so "UniswapX" or "1inch"
 *  reads as more than a label. */
function sourceDetail(source: string): string {
  if (source === "uniswapx")
    return "UniswapX — Dutch-auction intent order (V2 reactor)";
  if (source === "oneinch") return "1inch — Limit Order Protocol v4.1 order";
  if (source === "solvent")
    return "Solvent — submitted straight to our own endpoint";
  return source;
}

/** Why a trade earned nothing. When routing priced the delivery, say what it would have cost —
 *  "declined" alone gives the reader no way to tell a near miss from an empty book. The real reason
 *  the trade lifecycle recorded (an admission rule, a margin call, or the sim gate's actual on-chain
 *  revert text) comes first, since it is the specific answer; the sourcing comparison is the account
 *  behind it. */
function declineTag(trade: TradeRecord): string {
  const why = trade.declineReason ? ` — ${trade.declineReason}` : "";
  if (!trade.indicativeInput) return `not earned — ${trade.status}${why}`;
  return `not earned — ${trade.status}${why} · sourcing ${tokenText(trade.indicativeInput)} vs ${tokenText(trade.input)} paid`;
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
      value: numberText(stats?.blockHeight),
      sub: "live",
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
      { label: "Signature", value: trade.signaturePresent ? "Provided" : "—" },
      { label: "Source", value: sourceLabel(trade.source) },
      {
        label: "Sourcing cost",
        value: trade.indicativeInput ? tokenText(trade.indicativeInput) : "—",
      },
    ],
    profit: trade.surplus ? tokenText(trade.surplus) : "—",
    profitTag: ["declined", "failed"].includes(trade.status)
      ? declineTag(trade)
      : "route estimate · net of estimated gas",
    empty: trade.legs.length === 0,
    emptyText: "No maker legs recorded for this order.",
  };
}

/** The venue's accent, for the source pill — reuses the palette's own semantic colors rather than
 *  inventing new ones: `info` (blue) for the public UniswapX book, the brand lime for 1inch (the
 *  protocol this resolver's own on-chain filler was built for), neutral for our own endpoint. */
function sourceTone(source: string): { background: string; color: string } {
  if (source === "uniswapx")
    return { background: "var(--info-bg)", color: "var(--info-ink)" };
  if (source === "oneinch")
    return { background: "var(--lime-wash)", color: "var(--green-darkest)" };
  return { background: "var(--surface)", color: "var(--text-mid)" };
}

/** Real token icons, so a reader identifies the pair by its actual mark rather than decoding an
 *  address or two initials. `null` for anything outside this set — the badge falls back to a
 *  monogram rather than a broken image. */
const TOKEN_ICONS: Record<string, string> = {
  WETH: "/tokens/weth.png",
  USDC: "/tokens/usdc.png",
  USDT: "/tokens/usdt.png",
  DAI: "/tokens/dai.png",
  WBTC: "/tokens/wbtc.png",
  LINK: "/tokens/link.png",
  UNI: "/tokens/uni.png",
};

function tokenIcon(symbol: string): string | null {
  return TOKEN_ICONS[symbol.toUpperCase()] ?? null;
}

/** Real venue icons — the protocol's own real mark, sourced properly (CoinGecko's public asset
 *  CDN), not a reproduction drawn from memory. `null` for anything outside this set. */
const SOURCE_ICONS: Record<string, string> = {
  uniswapx: "/sources/uniswapx.png",
  oneinch: "/sources/1inch.png",
};

function sourceIcon(source: string): string | null {
  return SOURCE_ICONS[source] ?? null;
}

/** A token's at-a-glance badge letters — the fallback for a symbol with no real icon, so a reader
 *  can still tell the pair apart without decoding an address. */
function tokenMonogram(symbol: string): string {
  return symbol.slice(0, 2).toUpperCase();
}

/** What actually became of an order, one tier richer than the door verdict: whether it was ever
 *  attempted, and — once it was — whether it filled, declined, or reverted, with the real reason
 *  the trade lifecycle recorded (not a guess). Reuses `STATUS_TONE`'s existing palette. */
function orderState(order: ObservedOrder): {
  label: string;
  detail: string;
  tone: { background: string; color: string };
} {
  if (order.verdict !== "admitted") {
    return {
      label: "Refused",
      detail: order.reason ?? "refused at the door",
      tone: { background: "var(--surface)", color: "var(--text-mid)" },
    };
  }
  if (order.tradeStatus === "confirmed" || order.tradeStatus === "submitted") {
    // A submitted fill that passed the sim gate is broadcast to a real filler contract call —
    // by the time it's tracked here it has already cleared the on-chain profitability/delivery
    // checks. `tx_hash` is populated once confirmation tracking catches up; until then, the fill
    // itself is real even though the hash isn't shown yet.
    return {
      label: "Filled",
      detail: order.tradeTxHash
        ? shortHash(order.tradeTxHash)
        : "filled — confirming",
      tone: STATUS_TONE.confirmed,
    };
  }
  if (order.tradeStatus === "failed") {
    return {
      label: "Failed on-chain",
      detail: order.tradeDeclineReason ?? "reverted after submission",
      tone: STATUS_TONE.failed,
    };
  }
  if (order.tradeStatus === "declined") {
    return {
      label: "Declined",
      detail: order.tradeDeclineReason ?? "declined",
      tone: STATUS_TONE.declined,
    };
  }
  if (order.tradeStatus) {
    return {
      label: "Pending",
      detail: order.tradeStatus,
      tone: STATUS_TONE.pending,
    };
  }
  if (order.indicativeIn) {
    return {
      label: "Unprofitable",
      detail: "priced, not yet worth a reservation",
      tone: STATUS_TONE.declined,
    };
  }
  return {
    label: "Pricing…",
    detail: "admitted, not priced yet",
    tone: STATUS_TONE.pending,
  };
}

/** One observed order as a row, in the same shape `tradeRow` produces: a pair, the flow through
 *  it, the numbers that decide it, and a state pill — plus enough at a glance (token monograms, the
 *  source venue, and what really became of it) that one look tells the whole story. */
export function orderRow(
  order: ObservedOrder,
  tokenOf: (address: string) => { symbol: string; decimals: number },
) {
  const tin = tokenOf(order.tokenIn);
  const tout = tokenOf(order.tokenOut ?? "");
  const asked = order.requiredOut
    ? `${amountText(order.requiredOut, tout.decimals)} ${tout.symbol}`
    : "—";
  const best = order.indicativeIn
    ? `${amountText(order.indicativeIn, tin.decimals)} ${tin.symbol}`
    : "not priced";
  const state = orderState(order);
  return {
    id: order.orderHash,
    // Only a trade this order actually became has a detail page to route to.
    tradeId: order.tradeId,
    pair: `${tin.symbol}/${tout.symbol}`,
    tokenInSymbol: tin.symbol,
    tokenOutSymbol: tout.symbol,
    tokenInIcon: tokenIcon(tin.symbol),
    tokenOutIcon: tokenIcon(tout.symbol),
    tokenInMonogram: tokenMonogram(tin.symbol),
    tokenOutMonogram: tokenMonogram(tout.symbol),
    hashLabel: shortHash(order.orderHash),
    source: sourceLabel(order.source),
    sourceIcon: sourceIcon(order.source),
    sourceGlyph: sourceGlyph(order.source),
    sourceDetail: sourceDetail(order.source),
    sourceStyle: sourceTone(order.source),
    input: `${amountText(order.amountIn, tin.decimals)} ${tin.symbol}`,
    asked,
    best,
    state: state.label,
    detail: state.detail,
    stateStyle: state.tone,
  };
}

/** Order flow at a glance: how much the feed showed us, and why most of it was refused.
 *
 *  Deliberately an aggregate. The refused orders are the bulk of the feed and individually
 *  uninteresting — what matters is which rule refused them and how often, since that is the list
 *  of things standing between the resolver and more flow. */
export function orderFlow(orders: ObservedOrder[] | undefined) {
  const seen = orders?.length ?? 0;
  const admitted = orders?.filter((o) => o.verdict === "admitted").length ?? 0;
  const byReason = new Map<string, number>();
  for (const order of orders ?? []) {
    if (order.verdict === "admitted") continue;
    const reason = order.reason ?? "refused";
    byReason.set(reason, (byReason.get(reason) ?? 0) + 1);
  }
  return {
    seen,
    admitted,
    dropped: seen - admitted,
    admittedPct: seen === 0 ? "—" : `${((admitted / seen) * 100).toFixed(1)}%`,
    reasons: [...byReason.entries()]
      .sort((a, b) => b[1] - a[1])
      .map(([reason, count]) => ({
        reason,
        count,
        sharePct: seen === 0 ? 0 : (count / seen) * 100,
      })),
  };
}
