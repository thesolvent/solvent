import { curveMonotoneX, line as d3Line, area as d3Area } from "d3-shape";

import type { PairPricePoint } from "@/ports/positions";
import { LOCALE, percent } from "@/lib/format";

/** The plot is drawn in this space and stretched by the SVG's viewBox. */
const W = 1000;
const H = 320;
/** Headroom so the extremes are not drawn on the frame itself. */
const PAD = 14;

export type PriceSpan = "7d" | "3m" | "all";

/** The periods the server actually serves; `1d`/`1y` would each need a new backend window. */
export const PRICE_SPANS: { id: PriceSpan; label: string }[] = [
  { id: "7d", label: "7D" },
  { id: "3m", label: "3M" },
  { id: "all", label: "All" },
];

export interface PriceChartModel {
  /** Empty when there is nothing to draw, which the view renders as a notice. */
  linePath: string;
  areaPath: string;
  yTicks: { label: string; top: string }[];
  xTicks: { label: string; left: string }[];
  /** The most recent price, and where to rule it across the plot. */
  last: { label: string; top: string } | null;
  /** Move over the window, already signed and formatted. */
  change: string | null;
  changeUp: boolean;
  /** Spoken description, since the drawing itself is not readable. */
  summary: string;
}

/** Quote-per-base, at a precision that keeps small pairs legible. */
function priceText(value: number): string {
  const digits = value >= 1000 ? 0 : value >= 1 ? 2 : 6;
  return value.toLocaleString(LOCALE, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
}

function dayText(ms: number): string {
  return new Date(ms).toLocaleDateString(LOCALE, {
    month: "short",
    day: "numeric",
  });
}

/**
 * One pair's price history, as the chart draws it.
 *
 * A flat series is given a synthetic band rather than a zero-height range, so a pegged pair plots
 * as a line through the middle instead of collapsing onto the axis or dividing by zero.
 */
export function priceChart(
  points: PairPricePoint[],
  pairLabel: string,
): PriceChartModel {
  const usable = points.filter((p) => Number.isFinite(p.price) && p.price > 0);
  if (usable.length < 2) {
    return {
      linePath: "",
      areaPath: "",
      yTicks: [],
      xTicks: [],
      last: null,
      change: null,
      changeUp: true,
      summary: `No price history for ${pairLabel} in this period`,
    };
  }

  const prices = usable.map((p) => p.price);
  const lo = Math.min(...prices);
  const hi = Math.max(...prices);
  const mid = (lo + hi) / 2;
  // A flat series has no range to scale against; give it one so the line lands mid-plot.
  const span = hi - lo || Math.max(mid * 0.01, Number.EPSILON);
  const floor = hi === lo ? mid - span / 2 : lo;

  const first = usable[0];
  const latest = usable[usable.length - 1];
  const t0 = first.timestampMs;
  const tSpan = latest.timestampMs - t0 || 1;

  const x = (ms: number) => ((ms - t0) / tSpan) * W;
  const y = (price: number) =>
    H - PAD - ((price - floor) / span) * (H - PAD * 2);

  const xy = usable.map(
    (p) => [x(p.timestampMs), y(p.price)] as [number, number],
  );

  const linePath =
    d3Line<[number, number]>()
      .x((d) => d[0])
      .y((d) => d[1])
      .curve(curveMonotoneX)
      .context(null)(xy) ?? "";

  const areaPath =
    d3Area<[number, number]>()
      .x((d) => d[0])
      .y0(H)
      .y1((d) => d[1])
      .curve(curveMonotoneX)
      .context(null)(xy) ?? "";

  const yTicks = [0, 0.25, 0.5, 0.75, 1].map((f) => ({
    label: priceText(floor + span * f),
    top: `${(1 - f) * 100}%`,
  }));

  const xTicks = [0, 0.5, 1].map((f) => ({
    label: dayText(t0 + tSpan * f),
    left: `${f * 100}%`,
  }));

  const movePct = ((latest.price - first.price) / first.price) * 100;
  const changeUp = movePct >= 0;

  return {
    linePath,
    areaPath,
    yTicks,
    xTicks,
    last: {
      label: priceText(latest.price),
      top: `${((y(latest.price) / H) * 100).toFixed(3)}%`,
    },
    change: percent(movePct, { sign: "arrow" }),
    changeUp,
    summary:
      `${pairLabel} moved ${changeUp ? "up" : "down"} ` +
      `${percent(Math.abs(movePct))} over this period, from ` +
      `${priceText(first.price)} to ${priceText(latest.price)}, ` +
      `low ${priceText(lo)}, high ${priceText(hi)}`,
  };
}
