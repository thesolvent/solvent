import {
  strategyAllocator,
  type AllocationCurve,
  type ReserveAllocation,
  type StrategyAllocator,
} from "@solvent/sdk/construction";
import { MAX_UINT248, parseTokenAmount } from "@solvent/sdk/validation";
import { formatUnits } from "viem";

import { BAND_K0 } from "@/data";
import type {
  CreatePair,
  CreateToken,
  PairPricePoint,
} from "@/ports/positions";
import type { AppState, CreateSpan } from "@/state";

export type BandBounds = { bandMax: number; bandMin: number };

const MAX_CUSTOM_BOUND_PERCENT = 50;
export const MIN_PEGGED_BOUND_PERCENT = 0.01;

/**
 * Keeps the band legal: a minimum width, a 50% ceiling, and a soft snap onto the
 * ±0.01% / ±0.04% / ±0.1% presets. Pegged+symmetric mirrors whichever edge moved.
 */
export function clampBand(
  state: AppState,
  next: Partial<BandBounds>,
): BandBounds {
  const merged = { bandMax: state.bandMax, bandMin: state.bandMin, ...next };

  if (state.strategy === "Pegged" && state.pegSym) {
    const raw = next.bandMax ?? next.bandMin ?? merged.bandMax;
    const w = Math.min(
      MAX_CUSTOM_BOUND_PERCENT,
      Math.max(MIN_PEGGED_BOUND_PERCENT, Math.abs(raw)),
    );
    return { bandMax: w, bandMin: -w };
  }

  const MIN_WIDTH = 0.002;
  let hi = Math.max(MIN_WIDTH, merged.bandMax);
  let lo = Math.min(-MIN_WIDTH, merged.bandMin);
  for (const v of [0.01, 0.04, 0.1]) {
    if (Math.abs(hi - v) < v * 0.14) hi = v;
    if (Math.abs(-lo - v) < v * 0.14) lo = -v;
  }
  return {
    bandMax: Math.min(MAX_CUSTOM_BOUND_PERCENT, hi),
    bandMin: Math.max(-MAX_CUSTOM_BOUND_PERCENT, lo),
  };
}

export type Vol = {
  h: string;
  on: boolean;
  vol: string;
  when: string;
  price: number;
  x: number;
};

export type VolTip = {
  left: string;
  top: string;
  shift: string;
  px: string;
  vol: string;
  when: string;
};

export type Cross = {
  top: string;
  left: string;
  shift: string;
  price: string;
  pct: string;
  date: string;
};

export type StepView = {
  n: number;
  open: boolean;
  locked: boolean;
  panelFlex: string;
  panelBg: string;
  numFg: string;
  tickBg: string;
  barFg: string;
  cursor: string;
};

const SERIES_N = 110;
const RECOMMENDATION_COUNT = 6;
const VOLUME_BAR_COUNT = 52;

const SUPERSCRIPT: Record<string, string> = {
  "-": "⁻",
  "+": "⁺",
  "0": "⁰",
  "1": "¹",
  "2": "²",
  "3": "³",
  "4": "⁴",
  "5": "⁵",
  "6": "⁶",
  "7": "⁷",
  "8": "⁸",
  "9": "⁹",
};

/** Keep small reversed prices readable without losing ordinary market-price formatting. */
export function formatPrice(value: number): string {
  if (!Number.isFinite(value)) return "—";
  if (value === 0) return "0";
  const magnitude = Math.abs(value);
  if (magnitude < 0.0001) {
    const [coefficient, exponent] = value.toExponential(4).split("e");
    const power = [...exponent]
      .map((digit) => SUPERSCRIPT[digit] ?? digit)
      .join("");
    return `${coefficient}×10${power}`;
  }
  const decimals =
    magnitude < 0.01
      ? Math.min(8, Math.max(4, 4 - Math.floor(Math.log10(magnitude))))
      : magnitude < 10
        ? 4
        : 2;
  return value.toLocaleString("en-US", {
    minimumFractionDigits: decimals,
    maximumFractionDigits: decimals,
  });
}

type ChartPoint = PairPricePoint & {
  changePct: number;
  scalePct: number;
  x: number;
};

function priceScalePercent(price: number, midpoint: number): number {
  return Math.log(price / midpoint) * 100;
}

function rangePercentToScale(percent: number): number {
  return Math.log1p(percent / 100) * 100;
}

function scaleToRangePercent(scale: number): number {
  return Math.expm1(scale / 100) * 100;
}

function priceAtScale(midpoint: number, scale: number): number {
  return midpoint * Math.exp(scale / 100);
}

function chartPoints(
  history: readonly PairPricePoint[],
  mid: number,
  flipped: boolean,
): ChartPoint[] {
  const ordered = history
    .flatMap((point) => {
      const price = flipped ? 1 / point.price : point.price;
      return Number.isFinite(point.timestampMs) &&
        point.timestampMs >= 0 &&
        Number.isFinite(price) &&
        price > 0
        ? [{ ...point, price }]
        : [];
    })
    .sort((left, right) => left.timestampMs - right.timestampMs)
    .filter((point, index, points) =>
      index === points.length - 1
        ? true
        : point.timestampMs !== points[index + 1].timestampMs,
    );
  const first = ordered[0]?.timestampMs ?? 0;
  const last = ordered.at(-1)?.timestampMs ?? first;
  const duration = last - first;
  return ordered.map((point, index) => ({
    ...point,
    changePct: (point.price / mid - 1) * 100,
    scalePct: priceScalePercent(point.price, mid),
    x:
      duration > 0
        ? ((point.timestampMs - first) / duration) * 100
        : index
          ? 100
          : 0,
  }));
}

function downsample<T>(points: readonly T[], limit: number): T[] {
  if (points.length <= limit) return [...points];
  return Array.from(
    { length: limit },
    (_, index) =>
      points[Math.round((index / (limit - 1)) * (points.length - 1))],
  );
}

function volumeBuckets(
  points: readonly ChartPoint[],
  limit: number,
): ChartPoint[] {
  if (points.length <= limit) return [...points];
  const size = Math.ceil(points.length / limit);
  const buckets: ChartPoint[] = [];
  for (let start = 0; start < points.length; start += size) {
    const bucket = points.slice(start, start + size);
    const last = bucket.at(-1);
    if (!last) continue;
    buckets.push({
      ...last,
      volumeUsd: bucket.reduce(
        (total, point) => total + (point.volumeUsd ?? 0),
        0,
      ),
    });
  }
  return buckets;
}

function formatChartDate(timestampMs: number, span: CreateSpan): string {
  const date = new Date(timestampMs);
  return date.toLocaleDateString(
    "en-US",
    span === "All"
      ? { month: "short", year: "numeric" }
      : { month: "short", day: "numeric", year: "numeric" },
  );
}

function formatVolume(value: number): string {
  return `$${value.toLocaleString("en-US", {
    notation: "compact",
    maximumFractionDigits: 1,
  })}`;
}

type PairView = {
  source: CreatePair | undefined;
  a: string;
  b: string;
  mid: number;
  tvlUsd: number | undefined;
  type: string;
  band: number;
  fee: string;
  feeBps: number;
  vol: number;
  walA: number;
  walB: number;
  walARaw: bigint;
  walBRaw: bigint;
};

const EMPTY_PAIR: PairView = {
  source: undefined,
  a: "—",
  b: "—",
  mid: 1,
  tvlUsd: undefined,
  type: "Unknown",
  band: 0.1,
  fee: "0.00%",
  feeBps: 0,
  vol: 0.1,
  walA: 0,
  walB: 0,
  walARaw: 0n,
  walBRaw: 0n,
};

function percentFromBps(bps: number): string {
  return `${(bps / 100).toFixed(2)}%`;
}

function pairView(pair: CreatePair): PairView {
  return {
    source: pair,
    a: pair.base.symbol,
    b: pair.quote.symbol,
    mid: pair.mid,
    tvlUsd: pair.tvlUsd,
    type: pair.type,
    band: pair.defaultBandPct,
    fee: percentFromBps(pair.defaultFeeBps),
    feeBps: pair.defaultFeeBps,
    vol: pair.type === "Stable" ? 0.05 : 0.35,
    walA: pair.base.balance,
    walB: pair.quote.balance,
    walARaw: pair.base.balanceRaw,
    walBRaw: pair.quote.balanceRaw,
  };
}

function recommendPairs(catalog: readonly PairView[]) {
  return catalog
    .map((pair, catalogIndex) => ({ ...pair, catalogIndex }))
    .sort((left, right) => {
      const leftTvl = left.tvlUsd ?? Number.NEGATIVE_INFINITY;
      const rightTvl = right.tvlUsd ?? Number.NEGATIVE_INFINITY;
      return leftTvl === rightTvl
        ? left.catalogIndex - right.catalogIndex
        : rightTvl - leftTvl;
    })
    .slice(0, RECOMMENDATION_COUNT);
}

function shortAddress(address: string): string {
  return `${address.slice(0, 6)}…${address.slice(-4)}`;
}

function reserveAmount(value: string, decimals: number): bigint | undefined {
  try {
    return parseTokenAmount(value, decimals, "Deposit amount", MAX_UINT248);
  } catch {
    return undefined;
  }
}

function amountProblem(
  value: string,
  token: CreateToken | undefined,
): string | undefined {
  if (!token) return undefined;
  try {
    parseTokenAmount(
      value,
      token.decimals,
      `${token.symbol} amount`,
      MAX_UINT248,
    );
    return undefined;
  } catch (error) {
    return error instanceof Error
      ? error.message
      : "Enter a valid deposit amount";
  }
}

function decimalString(value: number, decimals: number): string {
  const fixed = value.toFixed(decimals);
  return fixed.includes(".")
    ? fixed.replace(/(?:\.0+|(?<=[0-9])0+)$/, "").replace(/\.$/, "")
    : fixed;
}

type AmountFields = Pick<AppState, "amtA" | "amtB">;

function formatAllocation(
  allocation: ReserveAllocation,
  decimalsA: number,
  decimalsB: number,
): AmountFields {
  return {
    amtA: formatUnits(allocation.base, decimalsA),
    amtB: formatUnits(allocation.quote, decimalsB),
  };
}

function amountControls(
  allocator: StrategyAllocator | undefined,
  tokenA: CreateToken | undefined,
  tokenB: CreateToken | undefined,
  wallet: { a: number; b: number; rawA: bigint; rawB: bigint },
) {
  const unavailable = { amtA: "0", amtB: "0" };
  if (!allocator || !tokenA || !tokenB) {
    return {
      amountsFromA: (amtA: string): AmountFields => ({ amtA, amtB: "0" }),
      amountsFromB: (amtB: string): AmountFields => ({ amtA: "0", amtB }),
      halfFromA: unavailable,
      maxFromA: unavailable,
      halfFromB: unavailable,
      maxFromB: unavailable,
      maxAmounts: unavailable,
    };
  }

  const fromA = (amtA: string): AmountFields => {
    const reserve = reserveAmount(amtA, tokenA.decimals);
    return reserve === undefined
      ? { amtA, amtB: "0" }
      : {
          amtA,
          amtB: formatUnits(allocator.fromBase(reserve).quote, tokenB.decimals),
        };
  };
  const fromB = (amtB: string): AmountFields => {
    const reserve = reserveAmount(amtB, tokenB.decimals);
    return reserve === undefined
      ? { amtA: "0", amtB }
      : {
          amtA: formatUnits(allocator.fromQuote(reserve).base, tokenA.decimals),
          amtB,
        };
  };
  const walletA = formatUnits(wallet.rawA, tokenA.decimals);
  const walletB = formatUnits(wallet.rawB, tokenB.decimals);
  const available = {
    base: wallet.rawA,
    quote: wallet.rawB,
  };

  return {
    amountsFromA: fromA,
    amountsFromB: fromB,
    halfFromA: fromA(formatUnits(wallet.rawA / 2n, tokenA.decimals)),
    maxFromA: fromA(walletA),
    halfFromB: fromB(formatUnits(wallet.rawB / 2n, tokenB.decimals)),
    maxFromB: fromB(walletB),
    maxAmounts: formatAllocation(
      allocator.max(available),
      tokenA.decimals,
      tokenB.decimals,
    ),
  };
}

function feeBpsFromPercent(value: string): number | undefined {
  if (!/^\d{1,3}(?:\.\d{1,2})?$/.test(value)) return undefined;
  const percent = Number(value);
  const bps = Math.round(percent * 100);
  return bps <= 9_999 ? bps : undefined;
}

function percentageUsed(amount: bigint | undefined, balance: bigint): string {
  if (amount === undefined || balance === 0n) return "0%";
  const basisPoints = (amount * 10_000n) / balance;
  if (basisPoints > 1_000_000n) return ">10,000%";
  const value = Number(basisPoints) / 100;
  return `${Number.isInteger(value) ? value.toFixed(0) : value.toFixed(2)}%`;
}

function walletTokens(pairs: readonly CreatePair[]) {
  const byAddress = new Map<string, CreateToken>();
  for (const pair of pairs) {
    for (const token of [pair.base, pair.quote]) {
      const key = token.address.toLowerCase();
      const current = byAddress.get(key);
      if (!current || token.balanceRaw > current.balanceRaw)
        byAddress.set(key, token);
    }
  }
  return [...byAddress.values()].map((token) => ({
    sym: token.symbol,
    name: token.name,
    usd: token.valueUsd,
    bal: token.balance.toLocaleString("en-US", { maximumFractionDigits: 8 }),
    addr: shortAddress(token.address),
    chg: token.changePct,
    tags: token.tags,
    tint: token.tint,
  }));
}

export function createPosition(
  s: AppState,
  pairs: readonly CreatePair[],
  catalogProblem?: string,
  history: readonly PairPricePoint[] = [],
) {
  const catalog = pairs.map(pairView);
  const recommendations = recommendPairs(catalog);
  const pr = catalog[s.corePair] ?? catalog[0] ?? EMPTY_PAIR;
  const tokens = walletTokens(pairs);
  const ready = pr.source !== undefined;
  const flip = s.flipped;
  const tokenA = flip ? pr.source?.quote : pr.source?.base;
  const tokenB = flip ? pr.source?.base : pr.source?.quote;
  const A = flip ? pr.b : pr.a;
  const B = flip ? pr.a : pr.b;
  const mid = flip ? 1 / pr.mid : pr.mid;
  const wallet = flip
    ? { a: pr.walB, b: pr.walA, rawA: pr.walBRaw, rawB: pr.walARaw }
    : { a: pr.walA, b: pr.walB, rawA: pr.walARaw, rawB: pr.walBRaw };
  const full = s.strategy === "Full range";
  const pegged = s.strategy === "Pegged";

  const fmtPx = formatPrice;
  const pct = (v: number) => `${v > 0 ? "+" : ""}${v.toFixed(2)}%`;

  const chart = chartPoints(history, mid, flip);
  const chartExtent = chart.reduce(
    (extent, point) => Math.max(extent, Math.abs(point.scalePct)),
    0,
  );
  const defaultBandExtent = Math.max(
    Math.abs(rangePercentToScale(pr.band)),
    Math.abs(rangePercentToScale(-pr.band)),
  );
  const fitSpan = full
    ? Math.max(1.05, chartExtent * 1.1)
    : Math.max(0.1, defaultBandExtent * 2.6, chartExtent * 1.1);
  const halfSpan = fitSpan / s.chartZoom;
  const K = 50 / halfSpan;
  const hi = full ? scaleToRangePercent(halfSpan) : s.bandMax;
  const lo = full ? scaleToRangePercent(-halfSpan) : s.bandMin;
  const scaleHi = full ? halfSpan : rangePercentToScale(hi);
  const scaleLo = full ? -halfSpan : rangePercentToScale(lo);
  const bandScaleExtent = Math.max(Math.abs(scaleHi), Math.abs(scaleLo));

  // A log-price scale stays positive and makes reciprocal orientations visually symmetric.
  const yVbA = (scalePct: number) => 200 - scalePct * K * 4;
  const series = downsample(chart, SERIES_N)
    .map((point) =>
      [point.x * 10, Math.min(400, Math.max(0, yVbA(point.scalePct)))]
        .map((n) => n.toFixed(1))
        .join(","),
    )
    .join(" ");

  const pMax = mid * (1 + hi / 100);
  const pMin = mid * (1 + lo / 100);
  const inRange = mid <= pMax && mid >= pMin;
  const priceDecimals = Math.min(
    100,
    pegged || full
      ? (tokenB?.decimals ?? 18)
      : (tokenA?.decimals ?? 18) + (tokenB?.decimals ?? 18),
  );
  const spotPrice = decimalString(mid, priceDecimals);
  const priceMin = decimalString(pMin, priceDecimals);
  const priceMax = decimalString(pMax, priceDecimals);
  const allocationCurve: AllocationCurve = full
    ? { kind: "fullRange" }
    : pegged
      ? { kind: "pegged" }
      : { kind: "concentrated", priceMin, priceMax };
  let allocation: StrategyAllocator | undefined;
  if (ready && tokenA && tokenB) {
    try {
      allocation = strategyAllocator({
        base: tokenA,
        quote: tokenB,
        spotPrice,
        curve: allocationCurve,
      });
    } catch {
      allocation = undefined;
    }
  }
  const allocationProblem =
    ready && !allocation
      ? "Couldn’t calculate reserves at the live market price."
      : undefined;
  const amounts = amountControls(allocation, tokenA, tokenB, wallet);
  const oneBase = tokenA ? 10n ** BigInt(tokenA.decimals) : 0n;
  const bPerA =
    allocation && tokenB
      ? Number(formatUnits(allocation.fromBase(oneBase).quote, tokenB.decimals))
      : mid;

  const reserveA = tokenA && reserveAmount(s.amtA, tokenA.decimals);
  const reserveB = tokenB && reserveAmount(s.amtB, tokenB.decimals);
  const depositProblem =
    amountProblem(s.amtA, tokenA) ?? amountProblem(s.amtB, tokenB);
  const covA = percentageUsed(reserveA, wallet.rawA);
  const covB = percentageUsed(reserveB, wallet.rawB);
  const okA =
    reserveA !== undefined && reserveA > 0n && reserveA <= wallet.rawA;
  const okB =
    reserveB !== undefined && reserveB > 0n && reserveB <= wallet.rawB;
  const capped =
    (reserveA !== undefined && reserveA > wallet.rawA) ||
    (reserveB !== undefined && reserveB > wallet.rawB);
  const amountsMatch =
    allocation !== undefined &&
    reserveA !== undefined &&
    reserveB !== undefined &&
    allocation.matches({ base: reserveA, quote: reserveB });
  const sideOnly = !inRange;

  const feeOptions = [`Auto ${pr.fee}`, "0.01%", "0.05%", "0.30%", "Custom"];
  const activeFee = feeOptions.includes(s.createFee)
    ? s.createFee
    : feeOptions[0];
  const customFeeBps = feeBpsFromPercent(s.customFeePct);
  const selectedFeeBps = activeFee.startsWith("Auto")
    ? pr.feeBps
    : activeFee === "Custom"
      ? customFeeBps
      : Math.round(Number.parseFloat(activeFee) * 100);
  const feeProblem =
    selectedFeeBps === undefined ||
    !Number.isInteger(selectedFeeBps) ||
    selectedFeeBps < 0 ||
    selectedFeeBps > 9_999
      ? "Enter a fee from 0% to 99.99% with up to two decimal places."
      : undefined;
  const feeBps = selectedFeeBps ?? 0;
  const feeLabel =
    activeFee === "Custom" && customFeeBps !== undefined
      ? `${s.customFeePct}% custom`
      : activeFee;
  const curve = full
    ? "XYC (full range)"
    : pegged
      ? "Pegged"
      : "Concentrated {min, max}";

  const volumeSource = volumeBuckets(
    chart.filter((point) => point.volumeUsd !== undefined),
    Math.max(14, Math.round(VOLUME_BAR_COUNT / Math.sqrt(s.chartZoom))),
  );
  const maxVolume = volumeSource.reduce(
    (maximum, point) => Math.max(maximum, point.volumeUsd ?? 0),
    0,
  );
  const vols: Vol[] = volumeSource.map((point, index) => ({
    h: `${maxVolume > 0 ? Math.max(4, ((point.volumeUsd ?? 0) / maxVolume) * 100) : 0}%`,
    on: s.volHover === index,
    vol: formatVolume(point.volumeUsd ?? 0),
    when: formatChartDate(point.timestampMs, s.createSpan),
    price: point.price,
    x: point.x,
  }));

  let volTip: VolTip | null = null;
  if (s.volHover !== null && s.volHover !== undefined && vols[s.volHover]) {
    const hovered = vols[s.volHover];
    const xf = hovered.x / 100;
    volTip = {
      left: `${hovered.x.toFixed(2)}%`,
      top: `${Math.min(100, Math.max(0, yVbA(priceScalePercent(hovered.price, mid)) / 4)).toFixed(2)}%`,
      shift: xf > 0.7 ? "translate(-104%, -118%)" : "translate(4%, -118%)",
      px: fmtPx(hovered.price),
      vol: hovered.vol,
      when: hovered.when,
    };
  }

  const yOf = (scale: number) => 50 - scale * K;

  let cross: Cross | null = null;
  if (s.chartHover && chart.length > 0) {
    const i = Math.min(
      chart.length - 1,
      Math.max(0, Math.round((s.chartHover.x / 100) * (chart.length - 1))),
    );
    const point = chart[i];
    cross = {
      top: `${Math.min(100, Math.max(0, yVbA(point.scalePct) / 4)).toFixed(2)}%`,
      left: `${point.x.toFixed(2)}%`,
      shift:
        s.chartHover.x > 70
          ? "translate(-104%, -118%)"
          : "translate(4%, -118%)",
      price: fmtPx(point.price),
      pct: pct(point.changePct),
      date: formatChartDate(point.timestampMs, s.createSpan),
    };
  }

  const tickPoints = downsample(chart, 3);
  const timeTicks = tickPoints.map((point, index) => {
    const left =
      tickPoints.length === 1 ? 50 : 8 + (index / (tickPoints.length - 1)) * 84;
    return {
      key: point.timestampMs,
      label: formatChartDate(point.timestampMs, s.createSpan),
      left: `${left}%`,
      shift:
        index === 0
          ? "translateX(0)"
          : index === tickPoints.length - 1
            ? "translateX(-100%)"
            : "translateX(-50%)",
    };
  });

  const peggedBoundsSymmetric = Math.abs(hi + lo) < 0.0005;
  const symmetric = s.pegSym && peggedBoundsSymmetric;
  const canSubmit =
    ready &&
    okA &&
    okB &&
    !capped &&
    amountsMatch &&
    allocation !== undefined &&
    feeProblem === undefined &&
    (!pegged || peggedBoundsSymmetric);

  const steps: StepView[] = [1, 2, 3, 4].map((n) => {
    const locked = n === 4 && !canSubmit;
    const done =
      n < s.step && (n === 3 ? okA && okB && !capped && amountsMatch : true);
    const open = s.step === n;
    return {
      n,
      open,
      locked,
      panelFlex: open ? "1 1 auto" : "0 0 78px",
      panelBg: open
        ? "var(--paper)"
        : done
          ? "var(--lime-wash-mid)"
          : locked
            ? "var(--surface-alt)"
            : "var(--paper-soft)",
      numFg: open
        ? "var(--ink)"
        : done
          ? "#9fc95e"
          : locked
            ? "#dededa"
            : "var(--line-mid)",
      tickBg: done ? "var(--lime)" : "transparent",
      barFg: open
        ? "var(--ink)"
        : locked
          ? "var(--line-strong)"
          : "var(--text-mid)",
      cursor: locked ? "not-allowed" : "pointer",
    };
  });

  const walletRows = tokens.filter((t) => {
    const raw =
      s.pickerSlot === 2 ? (s.slotB ? "" : s.q2) : s.slotA ? "" : s.q1;
    const q = (raw || "").toLowerCase();
    const tagOk = !s.tokenTag || t.tags.includes(s.tokenTag);
    return (
      tagOk &&
      (!q ||
        t.sym.toLowerCase().includes(q) ||
        t.name.toLowerCase().includes(q) ||
        t.addr.toLowerCase().includes(q))
    );
  });

  return {
    pr,
    pair: pr.source,
    pairs: catalog,
    recommendations,
    A,
    B,
    mid,
    wallet,
    full,
    pegged,
    bPerA,
    ...amounts,
    fitSpan,
    bandScaleExtent,
    fmtPx,
    pct,

    pairType: `${pr.type} pair`,
    opening: fmtPx(mid),
    market: fmtPx(mid),
    quote: B,
    flipLabel: `${A} ⇄ ${B}`,

    tintA: s.slotA
      ? (tokens.find((t) => t.sym === s.slotA)?.tint ?? "#f1f1ee")
      : "#f1f1ee",
    tintB: s.slotB
      ? (tokens.find((t) => t.sym === s.slotB)?.tint ?? "#f1f1ee")
      : "#f1f1ee",
    slotHint: s.pickerSlot === 2 ? "Choosing token 2" : "Choosing token 1",
    pairWarn: (() => {
      if (!ready) return catalogProblem ?? "Loading supported pairs…";
      if (!s.slotA || !s.slotB) return "";
      if (s.slotA === s.slotB) return "Pick two different tokens.";
      const hit = catalog.findIndex(
        (c) =>
          (c.a === s.slotA && c.b === s.slotB) ||
          (c.a === s.slotB && c.b === s.slotA),
      );
      return hit > -1
        ? ""
        : `${s.slotA} / ${s.slotB} isn't a supported devnet pair yet.`;
    })(),
    walletRows: walletRows.map((t) => {
      const sel = s.slotA === t.sym || s.slotB === t.sym;
      const other =
        s.slotA && s.slotA !== t.sym
          ? s.slotA
          : s.slotB && s.slotB !== t.sym
            ? s.slotB
            : null;
      return {
        token: t,
        rowBg: sel ? "var(--lime-wash-mid)" : "var(--paper)",
        // Tokens that can't pair with the already-picked side fade back.
        dim:
          !other || sel
            ? 1
            : catalog.some(
                  (c) =>
                    (c.a === other && c.b === t.sym) ||
                    (c.a === t.sym && c.b === other),
                )
              ? 1
              : 0.38,
        mark: s.slotA === t.sym ? "1" : s.slotB === t.sym ? "2" : "",
        markBg: sel ? "var(--ink)" : "transparent",
        markFg: sel ? "var(--paper)" : "transparent",
        usd:
          t.usd === undefined
            ? "$—"
            : `$${t.usd.toLocaleString("en-US", { maximumFractionDigits: 2 })}`,
        amt: `${t.bal} ${t.sym}`,
        delta:
          t.chg === undefined
            ? "—"
            : `${t.chg > 0 ? "+" : ""}${t.chg.toFixed(2)}%`,
        deltaFg:
          t.chg === undefined
            ? "var(--text-muted)"
            : t.chg > 0
              ? "var(--green-deep)"
              : t.chg < 0
                ? "#c2564a"
                : "var(--text-muted)",
      };
    }),
    emptyList: walletRows.length === 0,

    isPegged: pegged,
    notFull: !full,
    pegSymLabel: symmetric ? "Symmetric" : "Asymmetric",
    pegSymBg: symmetric ? "var(--lime)" : "var(--paper)",
    symmetric,

    feeOptions,
    activeFee,
    feeLabel,
    feeProblem,

    bandTop: `${Math.max(0, yOf(scaleHi)).toFixed(2)}%`,
    bandBottom: `${Math.min(100, yOf(scaleLo)).toFixed(2)}%`,
    bandHeight: `${Math.min(100, Math.max(1.2, (scaleHi - scaleLo) * K)).toFixed(2)}%`,
    bandFill: inRange ? "rgba(200, 242, 78, 0.32)" : "rgba(18, 92, 74, 0.14)",
    edgeColor: inRange ? "var(--green-deep)" : "var(--ok-ink)",
    edgeW: s.dragging ? "3px" : "1.5px",
    bodyCursor: full ? "default" : "grab",
    ariaMax: `Max price, ${pct(hi)}`,
    ariaMin: `Min price, ${pct(lo)}`,
    maxPill: `Max ${pct(hi)} · ${fmtPx(pMax)}`,
    minPill: `Min ${pct(lo)} · ${fmtPx(pMin)}`,

    series,
    vols,
    volTip,
    cross,
    scaleK: K.toPrecision(12),
    bandScaleMax: scaleHi.toPrecision(12),
    bandScaleMin: scaleLo.toPrecision(12),
    zoomLabel: `${s.chartZoom < 10 ? s.chartZoom.toFixed(1) : Math.round(s.chartZoom)}×`,
    axisHi: fmtPx(priceAtScale(mid, halfSpan)),
    axisLo: fmtPx(priceAtScale(mid, -halfSpan)),
    axisMax: fmtPx(pMax),
    axisMin: fmtPx(pMin),
    axisMid: fmtPx(mid),
    timeTicks,

    covA: `uses ${covA} of balance`,
    covB: `uses ${covB} of balance`,
    stateA: okA
      ? "sufficient"
      : reserveA !== undefined && reserveA > wallet.rawA && tokenA
        ? `over by ${formatUnits(reserveA - wallet.rawA, tokenA.decimals)}`
        : "enter an amount",
    stateB: okB
      ? "sufficient"
      : reserveB !== undefined && reserveB > wallet.rawB && tokenB
        ? `over by ${formatUnits(reserveB - wallet.rawB, tokenB.decimals)}`
        : "enter an amount",
    bdA: okA ? "var(--line)" : "var(--ok-ink)",
    bdB: okB ? "var(--line)" : "var(--ok-ink)",
    tagA: okA ? "var(--lime-wash-soft)" : "var(--ok-bg)",
    tagB: okB ? "var(--lime-wash-soft)" : "var(--ok-bg)",
    fgA: okA ? "var(--green-darkest)" : "var(--ok-ink-deep)",
    fgB: okB ? "var(--green-darkest)" : "var(--ok-ink-deep)",
    walletA: tokenA ? formatUnits(wallet.rawA, tokenA.decimals) : "0",
    walletB: tokenB ? formatUnits(wallet.rawB, tokenB.decimals) : "0",

    rangeTag: full ? "Full range" : inRange ? "In range" : "Out of range",
    rangeTagBg: full
      ? "var(--line-faint)"
      : inRange
        ? "var(--lime-wash-soft)"
        : "var(--ok-bg)",
    rangeTagFg: full
      ? "var(--text-strong)"
      : inRange
        ? "var(--green-darkest)"
        : "var(--ok-ink-deep)",
    curveLabel: `Curve → ${curve}`,
    pairNote: `1 ${A} pairs with ${fmtPx(bPerA)} ${B} at the live market price`,

    spotPrice,
    priceMin,
    priceMax,
    halfWidthPct: Math.max(Math.abs(hi), Math.abs(lo)),
    feeBps,
    peggedSymmetric: peggedBoundsSymmetric,

    cta: !ready
      ? "Loading supported pairs"
      : allocationProblem
        ? "Couldn’t calculate deposit amounts"
        : feeProblem
          ? "Enter a valid custom fee"
          : depositProblem
            ? "Enter a valid deposit amount"
            : pegged && !peggedBoundsSymmetric
              ? "Pegged range must be symmetric"
              : capped
                ? "Amount exceeds wallet balance"
                : !(okA && okB)
                  ? "Enter a deposit amount"
                  : !amountsMatch
                    ? "Recalculate deposit amounts"
                    : `Approve ${A} & ${B} — step 1 of 2`,
    ctaDisabled: !canSubmit,
    ctaBg: canSubmit ? "var(--green)" : "#ecece7",
    ctaFg: canSubmit ? "var(--paper)" : "var(--text-muted)",
    ctaCursor: canSubmit ? "pointer" : "not-allowed",
    footFg:
      allocationProblem ||
      feeProblem ||
      depositProblem ||
      capped ||
      !amountsMatch ||
      (pegged && !peggedBoundsSymmetric)
        ? "var(--ok-ink-deep)"
        : "var(--text-muted)",
    footNote:
      allocationProblem ??
      feeProblem ??
      depositProblem ??
      (pegged && !peggedBoundsSymmetric
        ? "Aqua pegged positions use one symmetric width. Link the bounds or choose a symmetric preset."
        : okA && okB && !capped && !amountsMatch
          ? "Deposit amounts must match the selected curve at the live market price."
          : s.step === 4 && !capped && inRange && okA && okB
            ? ""
            : capped
              ? "Reduce the amount — deposits are capped to your wallet balance."
              : sideOnly
                ? `Market sits outside your range: the position ships single-sided and stays inactive until price re-enters [${fmtPx(pMin)}, ${fmtPx(pMax)}].`
                : "Immutable: a shipped position can't be edited. Dock and ship a new one to change it."),

    steps,
    backVis: s.step > 1 ? "visible" : "hidden",
    nextLabel: s.step === 3 ? "Review" : "Next step",
    paneTitle: [
      "Pick a pair",
      "Set your active price",
      "Fee & deposit",
      "Review & create",
    ][Math.max(0, s.step - 1)],
    recap: [
      { label: "Pair", value: `${A} / ${B}` },
      {
        label: "Curve",
        value: full ? "XYC (full range)" : pegged ? "Pegged" : "Concentrated",
      },
      {
        label: "Range",
        value: full ? "full" : `${fmtPx(pMin)} – ${fmtPx(pMax)}`,
      },
      { label: "Fee", value: feeLabel },
      {
        label: "Deposit",
        value: `${s.amtA} ${A} + ${s.amtB} ${B}`,
      },
    ],
  };
}

export { BAND_K0 };
