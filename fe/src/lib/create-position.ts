import { BAND_K0, CORE6, TIME_LABELS, WALLET_TOKENS } from "@/data";
import type { AppState } from "@/state";

export type BandBounds = { bandMax: number; bandMin: number };

/**
 * Keeps the band legal: a minimum width, a 3.0 ceiling, and a soft snap onto the
 * ±1% / ±4% / ±10% presets. Pegged+symmetric mirrors whichever edge moved.
 */
export function clampBand(
  state: AppState,
  next: Partial<BandBounds>,
): BandBounds {
  const merged = { bandMax: state.bandMax, bandMin: state.bandMin, ...next };

  if (state.strategy === "Pegged" && state.pegSym) {
    const raw = next.bandMax ?? next.bandMin ?? merged.bandMax;
    const w = Math.min(3, Math.max(0.004, Math.abs(raw)));
    return { bandMax: w, bandMin: -w };
  }

  const MIN_WIDTH = 0.002;
  let hi = Math.max(MIN_WIDTH, merged.bandMax);
  let lo = Math.min(-MIN_WIDTH, merged.bandMin);
  for (const v of [0.01, 0.04, 0.1]) {
    if (Math.abs(hi - v) < v * 0.14) hi = v;
    if (Math.abs(-lo - v) < v * 0.14) lo = -v;
  }
  return { bandMax: Math.min(3, hi), bandMin: Math.max(-3, lo) };
}

export type Vol = { h: string; on: boolean; vol: string; when: string };

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

export function createPosition(s: AppState) {
  const pr = CORE6[s.corePair] ?? CORE6[0];
  const flip = s.flipped;
  const A = flip ? pr.b : pr.a;
  const B = flip ? pr.a : pr.b;
  const mid = flip ? 1 / pr.mid : pr.mid;
  const wallet = flip ? { a: pr.walB, b: pr.walA } : { a: pr.walA, b: pr.walB };
  const full = s.strategy === "Full range";
  const pegged = s.strategy === "Pegged";

  const dp = (v: number) => (v < 10 ? 4 : 2);
  const fmtPx = (v: number) =>
    v.toLocaleString("en-US", {
      minimumFractionDigits: dp(v),
      maximumFractionDigits: dp(v),
    });
  const pct = (v: number) => `${v > 0 ? "+" : ""}${v.toFixed(2)}%`;

  const winAmp =
    ({ "7d": 0.45, "3m": 1, All: 1.8 } as Record<string, number>)[
      s.createSpan
    ] ?? 1;
  const amp = pr.vol * winAmp;
  const seed = s.corePair * 3.1 + (flip ? 1.7 : 0);
  const rawSeries = Array.from({ length: SERIES_N }, (_, i) => {
    const t = i / (SERIES_N - 1);
    return (
      (Math.sin(t * 9 + seed) * 0.44 +
        Math.sin(t * 3.1 + seed * 1.4) * 0.36 +
        Math.sin(t * 21 + seed) * 0.12) *
      amp
    );
  });
  const anchorPct = rawSeries[SERIES_N - 1];

  const fitSpan = full ? 1.05 : Math.max(pr.band * 2.6, pr.vol * 1.4);
  const halfSpan = fitSpan / s.chartZoom;
  const K = 50 / halfSpan;
  const hi = full ? halfSpan : s.bandMax;
  const lo = full ? -halfSpan : s.bandMin;
  const span = hi - lo;

  // One price->y transform shared by the series, the band and the axis.
  const yVbA = (vPct: number) => 200 - (vPct - anchorPct) * K * 4;
  const series = rawSeries
    .map((v, i) =>
      [(i / (SERIES_N - 1)) * 1000, Math.min(400, Math.max(0, yVbA(v)))]
        .map((n) => n.toFixed(1))
        .join(","),
    )
    .join(" ");

  const last = mid * (1 + anchorPct / 100);
  const pMax = mid * (1 + (anchorPct + hi) / 100);
  const pMin = mid * (1 + (anchorPct + lo) / 100);
  const inRange = last <= pMax && last >= pMin;

  // The band shapes the token split: more of it above market means quote-heavy.
  // It pairs the two sides; it never rewrites what you deposit.
  const ratio = full
    ? 0.5
    : Math.min(0.98, Math.max(0.02, (pMax - last) / (pMax - pMin || 1)));
  const bPerA = last * (ratio ? (1 - ratio) / ratio : 1);

  const a = parseFloat(s.amtA) || 0;
  const b = parseFloat(s.amtB) || 0;
  const covA = wallet.a ? Math.round((a / wallet.a) * 100) : 0;
  const covB = wallet.b ? Math.round((b / wallet.b) * 100) : 0;
  const okA = a > 0 && a <= wallet.a;
  const okB = b > 0 && b <= wallet.b;
  const capped = a > wallet.a || b > wallet.b;
  const sideOnly = !inRange;

  const feeOptions = [`Auto ${pr.fee}`, "0.01%", "0.05%", "0.30%"];
  const activeFee = feeOptions.includes(s.createFee)
    ? s.createFee
    : feeOptions[0];
  const curve = full
    ? "XYC (full range)"
    : pegged
      ? "Pegged"
      : "Concentrated {min, max}";

  const VN = Math.max(14, Math.round(52 / Math.sqrt(s.chartZoom)));
  const dayStep =
    ({ "7d": 3.2, "3m": 42, All: 365 } as Record<string, number>)[
      s.createSpan
    ] ?? 42;
  const vols: Vol[] = Array.from({ length: VN }, (_, i) => {
    const d = new Date(2026, 8, 4, 14, 0);
    d.setDate(d.getDate() - Math.round(((VN - 1 - i) * dayStep) / 4));
    d.setHours(2 + ((i * 7) % 22));
    const wave = Math.abs(Math.sin(i * 1.21 * (52 / VN) + seed));
    return {
      h: `${(14 + wave * 72 + (i % 6) * 3).toFixed(0)}%`,
      on: s.volHover === i,
      vol: `$${((0.6 + wave * 4.2) * (52 / VN)).toFixed(1)}M`,
      when: `${d.toLocaleDateString("en-US", { month: "short", day: "numeric" })} · ${d.toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" })}`,
    };
  });

  let volTip: VolTip | null = null;
  if (s.volHover !== null && s.volHover !== undefined && vols[s.volHover]) {
    const xf = (s.volHover + 0.5) / VN;
    const si = Math.min(
      SERIES_N - 1,
      Math.max(0, Math.round(xf * (SERIES_N - 1))),
    );
    volTip = {
      left: `${(xf * 100).toFixed(2)}%`,
      top: `${Math.min(100, Math.max(0, yVbA(rawSeries[si]) / 4)).toFixed(2)}%`,
      shift: xf > 0.7 ? "translate(-104%, -118%)" : "translate(4%, -118%)",
      px: fmtPx(mid * (1 + rawSeries[si] / 100)),
      vol: vols[s.volHover].vol,
      when: vols[s.volHover].when,
    };
  }

  const yOf = (p: number) => 50 - p * K;

  let cross: Cross | null = null;
  if (s.chartHover) {
    const i = Math.min(
      SERIES_N - 1,
      Math.max(0, Math.round((s.chartHover.x / 100) * (SERIES_N - 1))),
    );
    const vPct = rawSeries[i];
    cross = {
      top: `${Math.min(100, Math.max(0, yVbA(vPct) / 4)).toFixed(2)}%`,
      left: `${((i / (SERIES_N - 1)) * 100).toFixed(2)}%`,
      shift:
        s.chartHover.x > 70
          ? "translate(-104%, -118%)"
          : "translate(4%, -118%)",
      price: fmtPx(mid * (1 + vPct / 100)),
      pct: pct(vPct - anchorPct),
      date: TIME_LABELS[s.createSpan][
        Math.min(2, Math.floor(s.chartHover.x / 34))
      ],
    };
  }

  const symmetric = s.pegSym && Math.abs(s.bandMax + s.bandMin) < 0.0005;

  const steps: StepView[] = [1, 2, 3, 4].map((n) => {
    const locked = n === 4 && !(okA && okB && !capped);
    const done = n < s.step && (n === 3 ? okA && okB && !capped : true);
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

  const walletRows = WALLET_TOKENS.filter((t) => {
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
    A,
    B,
    mid,
    wallet,
    full,
    pegged,
    bPerA,
    fitSpan,
    fmtPx,
    pct,

    pairType: `${pr.type} pair`,
    opening: fmtPx(mid),
    market: fmtPx(last),
    quote: B,
    flipLabel: `${A} ⇄ ${B}`,

    tintA: s.slotA
      ? (WALLET_TOKENS.find((t) => t.sym === s.slotA)?.tint ?? "#f1f1ee")
      : "#f1f1ee",
    tintB: s.slotB
      ? (WALLET_TOKENS.find((t) => t.sym === s.slotB)?.tint ?? "#f1f1ee")
      : "#f1f1ee",
    slotHint: s.pickerSlot === 2 ? "Choosing token 2" : "Choosing token 1",
    pairWarn: (() => {
      if (!s.slotA || !s.slotB) return "";
      if (s.slotA === s.slotB) return "Pick two different tokens.";
      const hit = CORE6.findIndex(
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
            : CORE6.some(
                  (c) =>
                    (c.a === other && c.b === t.sym) ||
                    (c.a === t.sym && c.b === other),
                )
              ? 1
              : 0.38,
        mark: s.slotA === t.sym ? "1" : s.slotB === t.sym ? "2" : "",
        markBg: sel ? "var(--ink)" : "transparent",
        markFg: sel ? "var(--paper)" : "transparent",
        usd: `$${t.usd.toFixed(2)}`,
        amt: `${t.bal} ${t.sym}`,
        delta: `${t.chg > 0 ? "+" : ""}${t.chg.toFixed(2)}%`,
        deltaFg:
          t.chg > 0
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

    bandTop: `${Math.max(0, yOf(hi)).toFixed(2)}%`,
    bandBottom: `${Math.min(100, yOf(lo)).toFixed(2)}%`,
    bandHeight: `${Math.min(100, Math.max(1.2, span * K)).toFixed(2)}%`,
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
    scaleK: K.toFixed(4),
    zoomLabel: `${s.chartZoom < 10 ? s.chartZoom.toFixed(1) : Math.round(s.chartZoom)}×`,
    axisHi: fmtPx(mid * (1 + (anchorPct + 50 / K) / 100)),
    axisLo: fmtPx(mid * (1 + (anchorPct - 50 / K) / 100)),
    axisMax: fmtPx(pMax),
    axisMin: fmtPx(pMin),
    axisMid: fmtPx(last),
    timeTicks: TIME_LABELS[s.createSpan].map((t, i) => ({
      label: t,
      left: `${i === 0 ? 8 : i === 1 ? 50 : 92}%`,
      shift:
        i === 0
          ? "translateX(0)"
          : i === 1
            ? "translateX(-50%)"
            : "translateX(-100%)",
    })),

    covA: `uses ${covA}% of balance`,
    covB: `uses ${covB}% of balance`,
    stateA: okA
      ? "sufficient"
      : a > wallet.a
        ? `over by ${(a - wallet.a).toFixed(2)}`
        : "enter an amount",
    stateB: okB
      ? "sufficient"
      : b > wallet.b
        ? `over by ${(b - wallet.b).toFixed(2)}`
        : "enter an amount",
    bdA: okA ? "var(--line)" : "var(--ok-ink)",
    bdB: okB ? "var(--line)" : "var(--ok-ink)",
    tagA: okA ? "var(--lime-wash-soft)" : "var(--ok-bg)",
    tagB: okB ? "var(--lime-wash-soft)" : "var(--ok-bg)",
    fgA: okA ? "var(--green-darkest)" : "var(--ok-ink-deep)",
    fgB: okB ? "var(--green-darkest)" : "var(--ok-ink-deep)",
    walletA: wallet.a.toFixed(2),
    walletB: wallet.b.toFixed(2),

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
    pairNote: `1 ${A} pairs with ${bPerA.toFixed(bPerA < 10 ? 4 : 2)} ${B} at this range`,

    cta: capped
      ? "Amount exceeds wallet balance"
      : !(okA && okB)
        ? "Enter a deposit amount"
        : `Approve ${A} & ${B} — step 1 of 2`,
    ctaDisabled: capped || !(okA && okB),
    ctaBg: capped || !(okA && okB) ? "#ecece7" : "var(--green)",
    ctaFg: capped || !(okA && okB) ? "var(--text-muted)" : "var(--paper)",
    ctaCursor: capped || !(okA && okB) ? "not-allowed" : "pointer",
    footFg: capped ? "var(--ok-ink-deep)" : "var(--text-muted)",
    footNote:
      s.step === 4 && !capped && inRange && okA && okB
        ? ""
        : capped
          ? "Reduce the amount — deposits are capped to your wallet balance."
          : sideOnly
            ? `Market sits outside your range: the position ships single-sided and stays inactive until price re-enters [${fmtPx(pMin)}, ${fmtPx(pMax)}].`
            : "Immutable: a shipped position can't be edited. Dock and ship a new one to change it.",

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
      { label: "Fee", value: activeFee },
      {
        label: "Deposit",
        value: `${a.toFixed(2)} ${A} + ${b.toFixed(2)} ${B}`,
      },
    ],
  };
}

export { BAND_K0 };
