import type { AppState } from "@/state";
import type {
  ActivityBucket,
  InventoryAsset,
  MakerDashboard,
  MakerPeriod,
  MakerSettlement,
  Position,
  PositionBalance,
} from "@/data/makers";
import {
  DASH,
  LOCALE,
  count,
  percent as percentText,
  tokenAmount,
  truncateAddress,
  usd as usdText,
} from "@/lib/format";
import { tradeRow } from "./explorer";

export const SPANS = ["7D", "1M", "3M", "6M"] as const;

/** Shared by the latency polyline geometry and the `viewBox` it is plotted against. */
export const LATENCY_VIEWBOX_WIDTH = 420;
export const LATENCY_VIEWBOX_HEIGHT = 120;

export const PERIODS: Record<string, MakerPeriod> = {
  "7D": "7d",
  "1M": "1m",
  "3M": "3m",
  "6M": "6m",
};

export const usd = usdText;

export function percent(value: number | null | undefined): string {
  return percentText(value, { digits: 1 });
}

const ratioText = (value: number | null | undefined) =>
  value == null ? DASH : `${value.toFixed(2)}×`;
const deltaText = (value: number | null | undefined) =>
  percentText(value, { digits: 1, sign: "arrow" });

export function balanceText(balances: PositionBalance[]): string {
  return balances.length
    ? balances.map((b) => `${tokenAmount(b.display)} ${b.symbol}`).join(" · ")
    : DASH;
}

function positionRow(p: Position, span: string) {
  const first = p.committed[0];
  const second = p.committed[1];
  const split =
    p.committedUsd && first?.usd != null
      ? (first.usd / p.committedUsd) * 100
      : null;
  return {
    hash: p.hash,
    maker: p.maker,
    pair: p.pair,
    tokens: [p.base, p.quote],
    meta: `${p.curve} · ${p.feeBps / 100}% · ${p.pairType}`,
    cov: `${ratioText(p.coverage)} cov`,
    covNum: ratioText(p.coverage),
    width: p.rangeKind === "full" ? "Full" : "Bounded",
    widthBg:
      p.rangeKind === "full" ? "var(--surface)" : "var(--lime-wash-soft)",
    widthFg:
      p.rangeKind === "full" ? "var(--text-mid)" : "var(--green-darkest)",
    splitKnown: split != null,
    splitA: `${split ?? 0}%`,
    labelA: `${percent(split)} ${first?.symbol ?? ""}`,
    labelB: `${percent(split == null ? null : 100 - split)} ${second?.symbol ?? ""}`,
    stats: [
      { label: "Current balance", value: balanceText(p.committed) },
      { label: "Opening balance", value: balanceText(p.opening) },
      { label: `Fees ${span}`, value: usd(p.feesUsd) },
      { label: `APY ${span}`, value: percent(p.apyPct) },
      { label: "Volume", value: usd(p.volumeUsd) },
    ].map((stat, i) => ({
      ...stat,
      sep: i === 0 ? "transparent" : "var(--surface)",
    })),
  };
}

const TOKEN_TINTS: Record<string, string> = {
  USDC: "#e6f0fb",
  USDT: "#e2f4ef",
  WBTC: "#fbeee0",
  WETH: "#eeeef4",
};

function assetRow(asset: InventoryAsset, open: boolean) {
  const ratio =
    asset.wallet.usd && asset.shared.usd != null
      ? asset.shared.usd / asset.wallet.usd
      : null;
  return {
    address: asset.address,
    sym: asset.symbol,
    tint: TOKEN_TINTS[asset.symbol] ?? "var(--surface)",
    open,
    across: `Across ${asset.legs.length} ${asset.legs.length === 1 ? "position" : "positions"}`,
    wallet: usd(asset.wallet.usd),
    walletAmt: `${tokenAmount(asset.wallet.display)} ${asset.symbol}`,
    shared: usd(asset.shared.usd),
    sharedAmt: `${tokenAmount(asset.shared.display)} ${asset.symbol}`,
    fees: usd(asset.feesUsd),
    apy: percent(asset.apyPct),
    ratio: ratioText(ratio),
    legs: asset.legs.map((leg) => ({
      hash: leg.hash,
      pair: leg.pair,
      meta: `${leg.curve} · ${leg.feeBps / 100}%`,
      cur: `${tokenAmount(leg.current.display)} ${asset.symbol}`,
      curUsd: usd(leg.current.usd),
      op: `${tokenAmount(leg.opening.display)} ${asset.symbol}`,
      fees: usd(leg.feesUsd),
      apy: percent(leg.apyPct),
      cov: ratioText(leg.coverage),
    })),
  };
}

function shareChart(
  dashboard: MakerDashboard | undefined,
  hovered: number | null,
) {
  const total = dashboard?.pairFills;
  const filled = dashboard?.fills;
  const segments = [
    { label: "This maker", count: filled, color: "var(--lime)" },
    {
      label: "Other makers",
      count:
        total == null || filled == null
          ? undefined
          : Math.max(0, total - filled),
      color: "var(--ink)",
    },
  ];
  const circumference = 2 * Math.PI * 46;
  const arcLength = circumference * 0.75;
  let offset = 0;
  const shares = segments.map((segment, i) => {
    const share = total && segment.count != null ? segment.count / total : 0;
    const length = Math.max(0, share * arcLength - 7);
    const result = {
      ...segment,
      // Arcs are filtered before rendering, so a segment carries the index hover reads back.
      index: i,
      label: segment.label,
      value: total ? percent(share * 100) : DASH,
      dot: segment.color,
      op: hovered === null || hovered === i ? 1 : 0.4,
      dash: `${length.toFixed(1)} ${(circumference - length).toFixed(1)}`,
      offset: (-offset).toFixed(1),
    };
    offset += share * arcLength;
    return result;
  });
  const selected = hovered === null ? undefined : shares[hovered];
  return {
    trackDash: `${arcLength.toFixed(1)} ${(circumference * 0.25).toFixed(1)}`,
    arcs: shares.filter((share) => Number.parseFloat(share.dash) > 0),
    shares,
    donutLabel: `Fill share: ${shares
      .map((share) => `${share.label} ${share.value}`)
      .join(", ")}`,
    donutCap: selected?.label ?? "total",
    donutVal: count(selected ? selected.count : total),
    tip: !!selected,
    tipLabel: selected?.label ?? "",
    tipPct: selected?.value ?? "",
    tipAmt: `${count(selected?.count)} orders`,
  };
}

function bucketLabel(bucket: ActivityBucket, span: string): string {
  const date = new Date(bucket.from * 1000);
  return span === "7D"
    ? date
        .toLocaleDateString(LOCALE, { weekday: "short", timeZone: "UTC" })
        .slice(0, 1)
    : date.toLocaleDateString(LOCALE, {
        day: "numeric",
        month: "numeric",
        timeZone: "UTC",
      });
}

function bucketRange(bucket: ActivityBucket): string {
  const date = new Intl.DateTimeFormat("en-US", {
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  });
  return `${date.formatRange(new Date(bucket.from * 1000), new Date((bucket.to - 1) * 1000))} · UTC`;
}

function activityCharts(
  dashboard: MakerDashboard | undefined,
  state: AppState,
) {
  const buckets = dashboard?.activity ?? [];
  const peak = Math.max(0, ...buckets.map((b) => b.fills));
  const topIndex = buckets.findIndex((b) => b.fills === peak);
  const average = buckets.length
    ? buckets.reduce((sum, b) => sum + b.fills, 0) / buckets.length
    : 0;
  const maxLatency = Math.max(1200, ...buckets.map((b) => b.latencyMs ?? 0));
  const y = (latency: number) => 116 - (latency / maxLatency) * 112;
  // A period other than 7D returns a different bucket count; a fixed step would clip the tail
  // while the visible remainder still read as the whole period. A lone bucket sits centred.
  const frac = (i: number) =>
    buckets.length > 1 ? i / (buckets.length - 1) : 0.5;
  const points = buckets.map((bucket, i) => ({
    bucket,
    x: frac(i) * LATENCY_VIEWBOX_WIDTH,
    y: bucket.latencyMs == null ? null : y(bucket.latencyMs),
  }));
  // Empty buckets break the line: interpolating across them would invent measurements.
  const lines: string[] = [];
  let segment: string[] = [];
  for (const point of points) {
    if (point.y !== null)
      segment.push(`${point.x.toFixed(1)},${point.y.toFixed(1)}`);
    else if (segment.length) {
      lines.push(segment.join(" "));
      segment = [];
    }
  }
  if (segment.length) lines.push(segment.join(" "));
  const fillBucket = state.mkBar === null ? undefined : buckets[state.mkBar];
  const latencyBucket = state.mkLat === null ? undefined : buckets[state.mkLat];
  return {
    fills: count(dashboard?.fills),
    fillsDelta: deltaText(dashboard?.fillsChangePct),
    fillsTip: fillBucket
      ? {
          label: bucketRange(fillBucket),
          value: count(fillBucket.fills),
          detail: "fills",
        }
      : null,
    bars: buckets.map((bucket, i) => ({
      day: bucketLabel(bucket, state.mkSpan),
      label: `${bucketRange(bucket)}: ${count(bucket.fills)} fills`,
      h: `${peak ? (bucket.fills / peak) * 100 : 0}%`,
      bg:
        state.mkBar === i
          ? "var(--ink)"
          : i === topIndex
            ? "var(--lime)"
            : "#eeeeea",
      dayFg:
        state.mkBar === i || i === topIndex ? "var(--ink)" : "var(--text-dim)",
    })),
    fillsLabel: `Fills by ${state.mkSpan} bucket: ${
      buckets.map((b) => `${bucketRange(b)} ${count(b.fills)}`).join(", ") ||
      "no buckets"
    }`,
    avgTop: `${peak ? 100 - (average / peak) * 100 : 100}%`,
    avgVal: count(average),
    latency:
      dashboard?.latencyMs == null ? DASH : `${count(dashboard.latencyMs)} ms`,
    latDelta:
      dashboard?.previousLatencyMs == null
        ? "no previous fills"
        : `from ${count(dashboard.previousLatencyMs)} ms baseline`,
    latAxis: [count(maxLatency), count(maxLatency / 2), "0"],
    latLines: lines.map((line) =>
      line.includes(" ") ? line : `${line} ${line}`,
    ),
    latDays: buckets.map((b) => bucketLabel(b, state.mkSpan)),
    latencyLabel: `Fill latency p50 by ${state.mkSpan} bucket: ${
      buckets
        .filter((b) => b.latencyMs != null)
        .map((b) => `${bucketRange(b)} ${count(b.latencyMs)} ms`)
        .join(", ") || "no measured fills"
    }`,
    latencyTip:
      latencyBucket?.latencyMs != null
        ? {
            label: bucketRange(latencyBucket),
            value: `${count(latencyBucket.latencyMs)} ms`,
            detail: "p50",
          }
        : null,
    latPts: points.map(({ bucket, y: value }, i) => ({
      left: `${frac(i) * 100}%`,
      top: value === null ? null : `${(value / LATENCY_VIEWBOX_HEIGHT) * 100}%`,
      label: `${bucketRange(bucket)}: ${
        bucket.latencyMs == null
          ? "no fills"
          : `${count(bucket.latencyMs)} ms p50`
      }`,
    })),
  };
}

export function makerView(
  state: AppState,
  data: {
    address: string | undefined;
    dashboard: MakerDashboard | undefined;
    positions: Position[];
    inventory: InventoryAsset[];
    settlements: MakerSettlement[];
    rebateCount: number;
    notice: string | undefined;
  },
) {
  const d = data.dashboard;
  const span = state.mkSpan;
  const kpis = [
    {
      label: "Shared liquidity",
      term: "sharedLiquidity" as const,
      value: usd(d?.sharedLiquidityUsd),
      change: d?.liquidityChangePct,
    },
    {
      label: "Volume, total",
      value: usd(d?.volumeUsd),
      change: d?.volumeChangePct,
    },
    {
      label: "Wallet balance",
      term: "walletBalance" as const,
      value: usd(d?.walletUsd),
    },
    {
      label: "Pullable",
      term: "pullable" as const,
      value: usd(d?.pullableUsd),
    },
    {
      label: "Shared-liq ratio",
      term: "sharedLiqRatio" as const,
      value: ratioText(d?.coverage),
    },
    {
      label: "Active positions",
      term: "activePositions" as const,
      value: count(d?.activePositions),
    },
    {
      label: `Fees, ${span}`,
      value: usd(d?.feesUsd),
      change: d?.feesChangePct,
    },
  ];
  const settlements = data.settlements;
  return {
    addr: truncateAddress(data.address),
    span,
    tab: state.mkTab,
    kpis: kpis.map((k, i) => ({
      ...k,
      delta: "change" in k ? deltaText(k.change) : "",
      deltaFg:
        k.change != null && k.change < 0
          ? "var(--warn-ink)"
          : "var(--green-deep)",
      bg: i === 3 ? "var(--lime-wash-mid)" : "var(--paper)",
      sep: i === 0 ? "transparent" : "var(--line)",
    })),
    tabs: [
      { key: "Positions", label: "Positions" },
      { key: "Assets", label: "Assets" },
      { key: "Settlements", label: "Settlements" },
      { key: "Rebates", label: "Rebates" },
    ],
    tabNote:
      data.notice ??
      (state.mkTab === "Positions"
        ? `${count(d?.activePositions)} active · ${span}`
        : state.mkTab === "Assets"
          ? `${data.inventory.length} tokens committed`
          : state.mkTab === "Settlements"
            ? `${count(d?.fills)} fills · ${span}`
            : `${data.rebateCount} recent rebates`),
    positions: data.positions.map((p) => positionRow(p, span)),
    assets: data.inventory.map((asset, i) =>
      assetRow(asset, state.mkAsset === i),
    ),
    settlements: settlements.map((trade) => {
      const row = tradeRow(trade);
      return {
        trade: trade.id,
        pair: row.pair,
        blk: row.blockLabel,
        tx: row.transactionLabel,
        status: trade.status,
        inn: row.input,
        out: row.output,
        fee: usd(trade.feeUsd),
        share: percent(trade.sharePct),
        stBg: row.statusStyle.background,
        stFg: row.statusStyle.color,
      };
    }),
    // Null rather than the notice: a load that failed is not an insight, and the caller renders
    // the failure where a failure belongs.
    insight: d
      ? `This maker filled ${count(d.fills)} ${d.fills === 1 ? "order" : "orders"} in the last ${d.windowDays} days.`
      : null,
    ...shareChart(d, state.mkTip),
    ...activityCharts(d, state),
  };
}
