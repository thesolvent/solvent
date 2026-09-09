import { curveMonotoneX } from "d3-shape";
import { path } from "d3-path";
import type { DepthCurve, DepthLevel } from "@/data";

/** What one sampled tier costs, as the stat row reads it. */
export type ImpactReadout = {
  label: string;
  price: string;
  size: string;
};

/** A sampled impact tier; hovering one asks the curve for that point. */
export type ImpactStop = {
  label: string;
  /** Where the tier sits along the curve, as the hover position that lands on it. */
  frac: number;
};

export type DetailHover = {
  left: string;
  dotTop: string;
  shift: string;
  size: string;
  price: string;
  output: string;
  makers: string;
  dots: { bg: string }[];
};

export interface DepthChartModel {
  aggPath: string;
  yTicks: { label: string; top: string }[];
  xTicks: { label: string; left: string }[];
  hover: DetailHover | null;
  impacts: ImpactStop[];
  axisTitle: string;
  priceTitle: string;
  bestPrice: string;
  near: ImpactReadout;
  far: ImpactReadout;
  totalLiq: string;
}

type Point = [x: number, y: number];

/**
 * The points the curve is drawn through: one per segment midpoint, anchored at the first
 * segment's start and the last one's end.
 */
function curvePoints(
  segs: { from: number; to: number; price: number }[],
  X: (n: number) => number,
  Y: (n: number) => number,
): Point[] {
  if (!segs.length) return [];
  const last = segs[segs.length - 1];
  return [
    [X(segs[0].from), Y(segs[0].price)],
    ...segs.map((seg): Point => [X((seg.from + seg.to) / 2), Y(seg.price)]),
    [X(last.to), Y(last.price)],
  ];
}

/** One drawn span of the curve: a cubic with the two control points that bend it. */
interface Cubic {
  from: Point;
  c1: Point;
  c2: Point;
  to: Point;
}

/** Record D3's monotone geometry once for both drawing and hover placement. */
function cubics(pts: Point[]): Cubic[] {
  const spans: Cubic[] = [];
  let from: Point = [0, 0];
  const context = path();
  context.moveTo = (x, y) => {
    from = [x, y];
  };
  context.bezierCurveTo = (x1, y1, x2, y2, x, y) => {
    const to: Point = [x, y];
    spans.push({ from, c1: [x1, y1], c2: [x2, y2], to });
    from = to;
  };
  context.lineTo = (x, y) => {
    const to: Point = [x, y];
    spans.push({ from, c1: from, c2: to, to });
    from = to;
  };
  const curve = curveMonotoneX(context);
  curve.lineStart();
  for (const [x, y] of pts) curve.point(x, y);
  curve.lineEnd();
  return spans;
}

const AXIS_X = 0;
const AXIS_Y = 1;

const xy = ([x, y]: Point) => `${x.toFixed(1)} ${y.toFixed(1)}`;

function pathOf(spans: Cubic[]): string {
  if (!spans.length) return "";
  return spans.reduce(
    (d, { c1, c2, to }) => `${d} C ${xy(c1)}, ${xy(c2)}, ${xy(to)}`,
    `M ${xy(spans[0].from)}`,
  );
}

/** One axis of a cubic Bezier at `t`. */
function axisAt(span: Cubic, axis: 0 | 1, t: number): number {
  const s = 1 - t;
  return (
    s * s * s * span.from[axis] +
    3 * s * s * t * span.c1[axis] +
    3 * s * t * t * span.c2[axis] +
    t * t * t * span.to[axis]
  );
}

/** Halvings to invert x(t) — well past sub-pixel at any plot width. */
const SOLVE_STEPS = 24;
// Dots are decorative; the adjacent count retains the complete maker total.
const MAX_MAKER_DOTS = 8;

/** Height of the drawn curve at `x`, by solving the span that covers it for `t`. */
function heightAt(spans: Cubic[], x: number): number {
  if (!spans.length) return 0;
  if (x <= spans[0].from[AXIS_X]) return spans[0].from[AXIS_Y];

  const span = spans.find((s) => x <= s.to[AXIS_X]) ?? spans[spans.length - 1];
  let lo = 0;
  let hi = 1;
  for (let i = 0; i < SOLVE_STEPS; i++) {
    const t = (lo + hi) / 2;
    if (axisAt(span, AXIS_X, t) < x) lo = t;
    else hi = t;
  }
  return axisAt(span, AXIS_Y, (lo + hi) / 2);
}

/** Prices span pennies on stable pairs and thousands on majors, so precision follows magnitude. */
function priceLabel(price: number): string {
  if (price > 0 && price < 0.0001) {
    return price.toLocaleString("en-US", { maximumSignificantDigits: 4 });
  }
  const digits = price < 10 ? 4 : 2;
  return price.toLocaleString("en-US", {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
}

/**
 * Turn the sampled curve into segments priced at the margin.
 *
 * The server reports cumulative size and output at each impact tier, so the price of the next
 * unit across a slice is what it delivers over what it costs — not the blended price at its end.
 */
function samplesOf(samples: DepthLevel[]) {
  const levels: DepthLevel[] = [];
  const segments: { from: number; to: number; price: number }[] = [];
  let size = 0;
  let output = 0;
  for (const level of samples) {
    const width = level.sizeIn - size;
    const marginal = (level.output - output) / width;
    if (
      !Number.isFinite(level.sizeIn) ||
      !Number.isFinite(level.output) ||
      !Number.isFinite(level.price) ||
      !Number.isFinite(level.impactPct) ||
      !Number.isSafeInteger(level.makersUsed) ||
      level.makersUsed < 0 ||
      width <= 0 ||
      level.output <= output ||
      level.price <= 0 ||
      !Number.isFinite(marginal) ||
      marginal <= 0
    )
      continue;
    levels.push(level);
    segments.push({ from: size, to: level.sizeIn, price: marginal });
    size = level.sizeIn;
    output = level.output;
  }
  return { levels, segments, total: size };
}

export function depthChart({
  depth,
  baseSymbol: baseSym,
  quoteSymbol: quoteSym,
  hoverFrac,
}: {
  depth: DepthCurve | undefined;
  baseSymbol: string;
  quoteSymbol: string;
  hoverFrac: number | null;
}): DepthChartModel {
  const amount = new Intl.NumberFormat("en-US", { maximumSignificantDigits: 6 })
    .format;
  const amountLabel = (value: number, symbol: string) =>
    `${amount(value)} ${symbol}`;
  const { levels, segments, total } = samplesOf(depth?.levels ?? []);
  const totalSize = total || 1;

  const prices = segments.map((seg) => seg.price);
  const best = depth?.bestPrice;
  const validBest =
    best != null && Number.isFinite(best) && best > 0 ? best : null;
  const span = [validBest ?? 0, ...prices].filter((price) => price > 0);
  const pMax = span.length
    ? Math.min(Number.MAX_VALUE, Math.max(...span) * 1.0005)
    : 1;
  const pMin = span.length ? Math.min(...span) * 0.9995 : 0;

  const X = (sz: number) => (sz / totalSize) * 1000;
  const Y = (pr: number) =>
    pMax === pMin ? 195 : 360 - ((pr - pMin) / (pMax - pMin)) * 330;
  const spans = cubics(curvePoints(segments, X, Y));

  const yTicks = Array.from({ length: 5 }, (_, i) => {
    const price = pMin + (pMax - pMin) * (i / 4);
    return {
      label: priceLabel(price),
      top: `${((Y(price) / 400) * 100).toFixed(2)}%`,
    };
  });
  const xTicks = Array.from({ length: 5 }, (_, i) => ({
    label: amount(totalSize * (i / 4)),
    left: `${((i / 4) * 100).toFixed(1)}%`,
  }));

  // Walk the curve to the hovered size so the tooltip reports the blended price paid, not the
  // marginal one at that point.
  let hover: DetailHover | null = null;
  if (hoverFrac != null && Number.isFinite(hoverFrac) && segments.length) {
    const target = Math.min(1, Math.max(0, hoverFrac)) * totalSize;
    let spent = 0;
    let filled = 0;
    let last = segments[0].price;
    for (const seg of segments) {
      const take = Math.min(seg.to, target) - seg.from;
      if (take <= 0) break;
      spent += take * seg.price;
      filled += take;
      last = seg.price;
      if (seg.to >= target) break;
    }
    const effective = filled ? spent / filled : last;
    const reached =
      levels.find((level) => level.sizeIn >= target) ??
      levels[levels.length - 1];
    const makersUsed = reached?.makersUsed ?? 0;
    hover = {
      left: `${((target / totalSize) * 100).toFixed(2)}%`,
      dotTop: `${((heightAt(spans, X(target)) / 400) * 100).toFixed(2)}%`,
      shift: target / totalSize > 0.56 ? "translateX(-108%)" : "translateX(8%)",
      size: amountLabel(filled, baseSym),
      price: `${priceLabel(effective)} ${quoteSym}`,
      output: amountLabel(spent, quoteSym),
      makers: String(makersUsed),
      dots: Array.from(
        { length: Math.min(makersUsed, MAX_MAKER_DOTS) },
        () => ({ bg: "var(--green)" }),
      ),
    };
  }

  // The server samples fixed impact tiers, so read the readouts off it rather than re-deriving.
  const tier = (pct: number) =>
    levels.reduce<DepthLevel | undefined>(
      (best, level) =>
        best === undefined ||
        Math.abs(level.impactPct - pct) < Math.abs(best.impactPct - pct)
          ? level
          : best,
      undefined,
    );
  const impact = (level: DepthLevel | undefined) => ({
    label: level ? `${level.impactPct.toFixed(1)}% impact` : "impact",
    price: level ? `~${priceLabel(level.price)}` : "—",
    size: level ? `~${amountLabel(level.sizeIn, baseSym)}` : "—",
  });
  // The tiers double as marker positions: hovering one drives the same marker the pointer does.
  const impacts: ImpactStop[] = levels.map((level) => ({
    label: `${level.impactPct.toFixed(1)}%`,
    frac: totalSize > 0 ? level.sizeIn / totalSize : 0,
  }));

  return {
    aggPath: pathOf(spans),
    yTicks: levels.length ? yTicks : [],
    xTicks: levels.length ? xTicks : [],
    hover,
    impacts,
    axisTitle: depth?.axisTitle ?? `Cumulative ${baseSym} available`,
    priceTitle: `${quoteSym} per ${baseSym}`,
    bestPrice: validBest != null ? priceLabel(validBest) : "—",
    near: impact(tier(0.5)),
    far: impact(tier(1)),
    totalLiq: levels.length ? amountLabel(totalSize, baseSym) : "—",
  };
}
