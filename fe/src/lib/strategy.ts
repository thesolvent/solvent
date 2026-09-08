import type { Position, PositionHistory } from "@/data/makers";
import type { TradeRecord } from "@/data/explorer";
import { balanceText, compactAddress, percent, usd } from "./makers";
import { relativeTime, tokenText } from "./explorer";

const priceText = (value: number | null | undefined) =>
  value == null || !Number.isFinite(value)
    ? "—"
    : value.toLocaleString("en-US", { maximumSignificantDigits: 7 });

export function strategyChart(
  position: Position | undefined,
  history: PositionHistory | undefined,
) {
  const lower = position?.lowerPrice;
  const upper = position?.upperPrice;
  const prices = history?.prices ?? [];
  const values = [
    lower,
    upper,
    position?.spot == null ? null : Number(position.spot),
    ...prices.map((p) => p.price),
  ].filter(
    (value): value is number =>
      value != null && Number.isFinite(value) && value > 0,
  );
  const low = values.length ? Math.min(...values) : 0;
  const high = values.length ? Math.max(...values) : 1;
  const pad = Math.max((high - low) * 0.08, high * 0.005);
  const bottom = Math.max(0, low - pad);
  const top = high + pad;
  const y = (price: number) => 300 - ((price - bottom) / (top - bottom)) * 300;
  const x = (at: number) =>
    history && history.to > history.from
      ? (640 * (at - history.from)) / (history.to - history.from)
      : 0;
  const lines: string[] = [];
  let points: string[] = [];
  let previous: number | null = null;
  for (const point of prices) {
    // Between reserve-changing transactions the strategy price is constant, including before docking.
    if (previous != null) points.push(`${x(point.at)},${y(previous)}`);
    if (
      point.price == null ||
      !Number.isFinite(point.price) ||
      point.price <= 0
    ) {
      if (points.length) lines.push(points.join(" "));
      points = [];
      previous = null;
    } else {
      points.push(`${x(point.at)},${y(point.price)}`);
      previous = point.price;
    }
  }
  if (previous != null && history) points.push(`640,${y(previous)}`);
  if (points.length) lines.push(points.join(" "));
  const bounded = lower != null && upper != null;
  return {
    lines,
    bandY: bounded ? y(upper) : 0,
    bandY2: bounded ? y(lower) : 300,
    bandH: bounded ? Math.max(0, y(lower) - y(upper)) : 300,
    showBand: !!position && (bounded || position.rangeKind === "full"),
    showBounds: bounded,
    tickLo: values.length ? priceText(bottom) : "—",
    tickHi: values.length ? priceText(top) : "—",
  };
}

function cell(
  label: string,
  value: string,
  tag: string,
  tone: "ok" | "warn" | "n" = "n",
) {
  return {
    label,
    value,
    tag,
    tagBg:
      tone === "ok"
        ? "var(--lime-wash-soft)"
        : tone === "warn"
          ? "var(--warn-bg)"
          : "var(--line-faint)",
    tagFg:
      tone === "ok"
        ? "var(--green-darkest)"
        : tone === "warn"
          ? "var(--warn-ink)"
          : "var(--text-mid)",
  };
}

export function strategyDetail(
  position: Position | undefined,
  history: PositionHistory | undefined,
  trades: TradeRecord[],
  notice = "",
) {
  const docked = position?.state === "docked";
  const full = position?.rangeKind === "full";
  const daily = position?.dailyFills ?? [];
  const maximum = Math.max(1, ...daily);
  return {
    ...strategyChart(position, history),
    title: position ? `${position.pair} · ${position.curve}` : "Strategy",
    state: position?.state ?? "—",
    stBg: docked ? "var(--line-faint)" : "var(--lime-wash-soft)",
    stFg: docked ? "var(--text-mid)" : "var(--green-darkest)",
    maker: compactAddress(position?.maker ?? ""),
    pool: position?.pair ?? "—",
    since:
      history?.createdBlock == null
        ? "—"
        : `since blk ${history.createdBlock.toLocaleString("en-US")}`,
    mid:
      position?.spot == null
        ? "—"
        : `${priceText(Number(position.spot))} ${position.quoteSymbol}`,
    bandNote:
      notice ||
      (full
        ? "full range — no bounds"
        : position?.curve === "Pegged"
          ? "peg-relative bounds"
          : position
            ? "bounded band"
            : "—"),
    axisFrom: "7d ago",
    axisTo: "now",
    spark: daily.map((count, i) => ({
      count,
      h: `${(count / maximum) * 100}%`,
      bg: docked
        ? "var(--surface)"
        : i === daily.length - 1
          ? "var(--lime)"
          : "#e2e2de",
    })),
    shape: [
      cell(
        "Virtual balance",
        balanceText(position?.committed ?? []),
        "committed",
      ),
      cell(
        "Actual / pullable",
        balanceText(position?.actual ?? []),
        position
          ? position.backed
            ? "fully backed ✓"
            : (position.shortfall ?? "partially backed")
          : "—",
        position ? (position.backed ? "ok" : "warn") : "n",
      ),
      cell("Fee", position ? `${position.feeBps} bps` : "—", "input fee"),
      cell(
        "Range",
        position?.range ?? "—",
        full
          ? "constant product"
          : position?.curve === "Pegged"
            ? "peg-relative bounds"
            : "bounded band",
      ),
    ].map((value, i) => ({ ...value, sep: i ? "var(--line)" : "transparent" })),
    active: [
      {
        label: "Fills (7d)",
        value: position?.fills7d?.toLocaleString("en-US") ?? "—",
        sub: docked ? "docked" : "pulls",
      },
      {
        label: "Volume (7d)",
        value: usd(position?.volume7dUsd),
        sub: "routed",
      },
      {
        label: "Quote uptime",
        value: percent(position?.quoteUptimePct),
        sub: docked ? "not quoting" : "7d",
      },
      {
        label: "Last fill",
        value: position?.lastFillAt ? relativeTime(position.lastFillAt) : "—",
        sub: "",
      },
    ].map((value, i) => ({ ...value, sep: i ? "var(--line)" : "transparent" })),
    fills: trades.map((trade) => ({
      id: trade.id,
      hash: trade.txHash ? compactAddress(trade.txHash) : "—",
      flow: `${tokenText(trade.input)} → ${tokenText(trade.output)}`,
      blk: trade.blockNumber?.toLocaleString("en-US") ?? "—",
      trade: `trade ${trade.id.slice(-6)}`,
    })),
    noFills: trades.length === 0,
  };
}
