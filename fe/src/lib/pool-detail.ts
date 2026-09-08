import type {
  DepthCurve,
  DepthLevel,
  Pool,
  PoolRoster,
  RosterMaker,
} from "@/data";

export type DetailMaker = {
  strategyHash: string;
  addr: string;
  curve: string;
  up: string;
  act: string;
  /** Teal when the maker quotes under its virtual size, lime when at full size. */
  gap: string;
  stateBg: string;
  stateFg: string;
};

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

export type PoolDetail = {
  pair: string;
  fee: string;
  apr: string;
  tvl: string;
  /** Absent while the move is unknown, so the badge can be left off. */
  tvlChange?: string;
  vol: string;
  fills: string;
  spread: string;
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
  makers: DetailMaker[];
  makerTotal: number;
};

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

/**
 * The Catmull-Rom cubics the curve is drawn from.
 *
 * The path and the hover marker both read these, so the marker cannot drift off the line the
 * way it does when one side smooths and the other reads the straight chord between samples.
 */
function cubics(pts: Point[]): Cubic[] {
  const spans: Cubic[] = [];
  for (let i = 0; i < pts.length - 1; i++) {
    const p0 = pts[Math.max(0, i - 1)];
    const p1 = pts[i];
    const p2 = pts[i + 1];
    const p3 = pts[Math.min(pts.length - 1, i + 2)];
    spans.push({
      from: p1,
      c1: [p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6],
      c2: [p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6],
      to: p2,
    });
  }
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

export interface PoolDetailInput {
  pool: Pool | undefined;
  roster: PoolRoster | undefined;
  depth: DepthCurve | undefined;
  /** Where along the curve the pointer is asking to look — over the plot, or over a tier. */
  hoverFrac: number | null;
  /** "Virtual" shows what each maker committed; "Actual" what it could deliver now. */
  makerSort: string;
}

/**
 * The roster, showing whichever balance the toggle asks for and ranked by it.
 *
 * A maker quoting less than it committed is short of its own position, which the row says by
 * carrying the warmer state colours rather than the lime ones.
 */
function rosterBy(
  roster: PoolRoster | undefined,
  deliverable: boolean,
): DetailMaker[] {
  const shown = (maker: RosterMaker) =>
    deliverable ? maker.actualUsd : maker.virtualUsd;

  return [...(roster?.makers ?? [])]
    .sort((a, b) => (shown(b) ?? -Infinity) - (shown(a) ?? -Infinity))
    .map((maker) => {
      const value = shown(maker);
      const short =
        maker.actualUsd != null &&
        maker.virtualUsd != null &&
        maker.actualUsd < maker.virtualUsd;
      return {
        strategyHash: maker.strategyHash,
        addr: maker.address,
        curve: maker.curve,
        // Quote uptime has no server source.
        up: "—",
        act: value == null ? "—" : USD.format(value),
        gap: short ? "var(--ok-ink)" : "var(--green)",
        stateBg: short ? "var(--ok-bg)" : "var(--lime-wash-soft)",
        stateFg: short ? "var(--ok-ink-deep)" : "var(--green-darkest)",
      };
    });
}

/** Prices span pennies on stable pairs and thousands on majors, so precision follows magnitude. */
function priceLabel(price: number): string {
  const digits = price < 10 ? 4 : 2;
  return price.toLocaleString("en-US", {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
}

const USD = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  notation: "compact",
  maximumFractionDigits: 1,
});

const amountLabel = (n: number, symbol: string) =>
  `${Math.round(n).toLocaleString("en-US")} ${symbol}`;

/**
 * Turn the sampled curve into segments priced at the margin.
 *
 * The server reports cumulative size and output at each impact tier, so the price of the next
 * unit across a slice is what it delivers over what it costs — not the blended price at its end.
 */
function segmentsOf(levels: DepthLevel[]) {
  const segments: { from: number; to: number; price: number }[] = [];
  let size = 0;
  let output = 0;
  for (const level of levels) {
    const width = level.sizeIn - size;
    if (width > 0)
      segments.push({
        from: size,
        to: level.sizeIn,
        price: (level.output - output) / width,
      });
    size = level.sizeIn;
    output = level.output;
  }
  return { segments, total: size };
}

export function poolDetail(input: PoolDetailInput): PoolDetail {
  const { pool, roster, depth, hoverFrac, makerSort } = input;
  const deliverable = makerSort === "Actual";
  const pair = pool?.pair ?? "";
  const [baseSym = "", quoteSym = ""] = pair.split(" / ");

  const levels = depth?.levels ?? [];
  const { segments, total } = segmentsOf(levels);
  const totalSize = total || 1;

  const prices = segments.map((seg) => seg.price);
  const best = depth?.bestPrice ?? prices[0] ?? 0;
  const span = [best, ...prices].filter((price) => price > 0);
  const pMax = span.length ? Math.max(...span) * 1.0005 : 1;
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
    label: Math.round(totalSize * (i / 4)).toLocaleString("en-US"),
    left: `${((i / 4) * 100).toFixed(1)}%`,
  }));

  // Walk the curve to the hovered size so the tooltip reports the blended price paid, not the
  // marginal one at that point.
  let hover: DetailHover | null = null;
  if (hoverFrac != null && segments.length) {
    const target = Math.max(totalSize * 0.02, hoverFrac * totalSize);
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
      dots: Array.from({ length: makersUsed }, () => ({ bg: "var(--green)" })),
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
    pair: pair || "—",
    fee: pool?.fee ?? "—",
    apr: pool?.apr ?? "—",
    tvl: pool?.tvl ?? "—",
    tvlChange: pool?.tvlChange,
    vol: pool?.vol ?? "—",
    fills: pool?.fills ?? "—",
    spread: (pool?.range ?? "").replace(" spread", "") || "—",
    aggPath: pathOf(spans),
    yTicks,
    xTicks,
    hover,
    impacts,
    axisTitle: depth?.axisTitle ?? `Cumulative ${baseSym} available`,
    priceTitle: `${quoteSym} per ${baseSym}`,
    bestPrice: depth?.bestPrice != null ? priceLabel(depth.bestPrice) : "—",
    near: impact(tier(0.5)),
    far: impact(tier(1)),
    totalLiq: levels.length ? amountLabel(totalSize, baseSym) : "—",
    makers: rosterBy(roster, deliverable),
    makerTotal: roster?.makers.length ?? 0,
  };
}
