import {
  ASSET_ROWS,
  DAY_LABELS,
  LAT_SERIES,
  MAKER_BARS,
  SHARE_SEGS,
  TRADES,
} from "@/data";
import type { AppState } from "@/state";

const DEFAULT_MAKER = "0x9f3c…MM-04";
const ROSTER = [DEFAULT_MAKER, "0x1a2b…MM-01", "0x77de…MM-07", "0xc410…MM-11"];
const SPANS = ["7D", "1M", "3M", "6M"];
const SPAN_MULT: Record<string, number> = {
  "7D": 0.42,
  "1M": 1,
  "3M": 2.6,
  "6M": 4.4,
};

/** Donut radius; the track is a 3/4 arc so the gap sits at the bottom. */
const R = 46;

const POSITIONS = [
  {
    pair: "USDC/USDT",
    curve: "Straight",
    fee: "0.04%",
    kind: "Pegged",
    cov: 1.0,
    wide: false,
    cur: "253.79 / 254.46",
    op: "253.79 / 254.46",
    fees: 0,
    apy: null as number | null,
    vol: 0,
    splitA: 50,
    a: "USDC",
    b: "USDT",
  },
  {
    pair: "WBTC/USDC",
    curve: "Concentrated",
    fee: "0.05%",
    kind: "Volatile",
    cov: 0.96,
    wide: true,
    cur: "0.0381 / 2,410.4",
    op: "0.0382 / 2,388.0",
    fees: 412,
    apy: 14.2,
    vol: 1.84e6,
    splitA: 62,
    a: "WBTC",
    b: "USDC",
  },
  {
    pair: "WETH/USDC",
    curve: "Concentrated",
    fee: "0.05%",
    kind: "Volatile",
    cov: 0.88,
    wide: false,
    cur: "1.204 / 3,702.1",
    op: "1.250 / 3,640.0",
    fees: 268,
    apy: 11.6,
    vol: 1.02e6,
    splitA: 44,
    a: "WETH",
    b: "USDC",
  },
];

const SETTLEMENTS = [
  {
    trade: 4,
    inn: "12,400 USDC",
    out: "12,398 USDT",
    fee: "+$4.96",
    share: "38%",
  },
  {
    trade: 1,
    inn: "0.0182 WBTC",
    out: "740 USDC",
    fee: "+$0.57",
    share: "12%",
  },
  {
    trade: 7,
    inn: "0.076 WBTC",
    out: "1.44 ETH",
    fee: "+$1.18",
    share: "19%",
  },
  {
    trade: 0,
    inn: "0.95 ETH",
    out: "2,942 USDC",
    fee: "+$3.25",
    share: "38%",
  },
  { trade: 5, inn: "0.62 WETH", out: "1,918 USDC", fee: "—", share: "44%" },
  {
    trade: 3,
    inn: "1.2 ETH",
    out: "3,713 USDC",
    fee: "+$0.29",
    share: "24%",
  },
];

const STATUS_TONE: Record<string, [string, string]> = {
  confirmed: ["var(--lime-wash-soft)", "var(--green-darkest)"],
  declined: ["var(--warn-bg)", "var(--warn-ink)"],
  "reorg-open": ["var(--info-bg)", "var(--info-ink)"],
  failed: ["var(--err-bg)", "var(--err-ink)"],
  pending: ["#eef0f4", "#4a5a72"],
  partial: ["var(--warn-bg)", "var(--warn-ink)"],
};

export { SPANS, ROSTER, DEFAULT_MAKER, SETTLEMENTS };

export function makerView(s: AppState) {
  const span = s.mkSpan;
  const mult = SPAN_MULT[span] ?? 1;
  const addr = s.mkSel ?? DEFAULT_MAKER;
  const money = (n: number) =>
    n >= 1e6 ? `$${(n / 1e6).toFixed(1)}M` : `$${(n / 1e3).toFixed(1)}k`;
  const feesRouted = Math.round(5080 * mult);

  // Bars are re-weighted per span so the peak (and the average line) move with it.
  const scaled = MAKER_BARS.map(
    (h, i) => h * (0.72 + Math.sin(i * 1.7 + mult) * 0.18 * mult),
  );
  const peak = Math.max(...scaled) || 1;
  const topIdx = scaled.indexOf(peak);
  const sum = scaled.reduce((x, y) => x + y, 0);
  const avg = sum / scaled.length;
  const base = (8204 * mult) / 7;

  const dimmed = (i: number) => s.mkTip === null || s.mkTip === i;

  return {
    addr,
    span,
    tab: s.mkTab,

    kpis: [
      {
        label: "Shared liquidity",
        value: "$18.4k",
        delta: "▲ 3.1%",
        deltaFg: "var(--green-deep)",
        bg: "var(--paper)",
      },
      {
        label: "Volume, total",
        value: money(6.2e6 * mult),
        delta: "▲ 8.4%",
        deltaFg: "var(--green-deep)",
        bg: "var(--paper)",
      },
      {
        label: "Wallet balance",
        value: "$21.0k",
        delta: "",
        deltaFg: "var(--text-muted)",
        bg: "var(--paper)",
      },
      {
        label: "Pullable",
        value: "$16.9k",
        delta: "",
        deltaFg: "var(--text-muted)",
        bg: "var(--lime-wash-mid)",
      },
      {
        label: "Shared-liq ratio",
        value: "0.96×",
        delta: "",
        deltaFg: "var(--text-muted)",
        bg: "var(--paper)",
      },
      {
        label: "Active positions",
        value: "3",
        delta: "",
        deltaFg: "var(--text-muted)",
        bg: "var(--paper)",
      },
      {
        label: `Fees, ${span}`,
        value: `$${feesRouted.toLocaleString("en-US")}`,
        delta: "▲ 2.6%",
        deltaFg: "var(--green-deep)",
        bg: "var(--paper)",
      },
    ].map((k, i) => ({
      ...k,
      sep: i === 0 ? "transparent" : "var(--line)",
    })),

    tabs: [
      { key: "Positions", label: `Positions ${POSITIONS.length}` },
      { key: "Assets", label: "Assets" },
      { key: "Settlements", label: "Settlements" },
    ],
    tabNote:
      s.mkTab === "Positions"
        ? `${POSITIONS.length} active · ${span}`
        : s.mkTab === "Assets"
          ? `${ASSET_ROWS.length} tokens committed`
          : `${Math.round(214 * mult)} fills · ${span}`,

    positions: POSITIONS.map((p, i) => {
      const open = s.mkOpen === i;
      return {
        pair: p.pair,
        meta: `${p.curve} · ${p.fee} · ${p.kind}`,
        cov: `${p.cov.toFixed(2)}× cov`,
        covNum: `${p.cov.toFixed(2)}×`,
        width: p.wide ? "Wide" : "Tight",
        widthBg: p.wide ? "var(--surface)" : "var(--lime-wash-soft)",
        widthFg: p.wide ? "var(--text-mid)" : "var(--green-darkest)",
        open,
        splitA: `${p.splitA}%`,
        labelA: `${p.splitA}% ${p.a}`,
        labelB: `${100 - p.splitA}% ${p.b}`,
        stats: [
          { label: "Current balance", value: p.cur },
          { label: "Opening balance", value: p.op },
          {
            label: `Fees ${span}`,
            value: `$${Math.round(p.fees * mult).toLocaleString("en-US")}`,
          },
          {
            label: `APY ${span}`,
            value: p.apy === null ? "—" : `${p.apy.toFixed(1)}%`,
          },
          {
            label: "Volume",
            value: p.vol ? money(p.vol * mult) : "$0",
          },
        ].map((st, j) => ({
          ...st,
          sep: j === 0 ? "transparent" : "var(--surface)",
        })),
      };
    }),

    assets: ASSET_ROWS.map((t, i) => ({
      sym: t.sym,
      tint: t.tint,
      across: `Across ${t.legs.length} ${t.legs.length === 1 ? "position" : "positions"}`,
      wallet: `$${Math.round(t.wallet).toLocaleString("en-US")}`,
      walletAmt: `${t.walletAmt} ${t.sym}`,
      shared: `$${Math.round(t.shared).toLocaleString("en-US")}`,
      sharedAmt: `${t.sharedAmt} ${t.sym}`,
      fees: `$${Math.round(t.fees * mult).toLocaleString("en-US")}`,
      apy: t.apy ? `${t.apy.toFixed(1)}%` : "–",
      ratio: `${(t.shared / t.wallet).toFixed(2)}×`,
      open: s.mkAsset === i,
      legs: t.legs.map((l) => ({
        pair: l.pair,
        meta: l.meta,
        cur: `${l.cur} ${t.sym}`,
        curUsd: `$${Math.round(l.usd).toLocaleString("en-US")}`,
        op: `${l.op} ${t.sym}`,
        fees: `$${Math.round(l.fees * mult).toLocaleString("en-US")}`,
        apy: l.apy ? `${l.apy.toFixed(1)}%` : "–",
        cov: `${l.cov.toFixed(2)}×`,
      })),
    })),

    settlements: SETTLEMENTS.map((x) => {
      const t = TRADES[x.trade];
      const [stBg, stFg] = STATUS_TONE[t.status];
      return {
        trade: x.trade,
        pair: t.pair,
        blk: t.blk === "—" ? "unsettled" : `blk ${t.blk}`,
        tx: t.tx,
        status: t.status,
        inn: x.inn,
        out: x.out,
        fee: x.fee,
        share: t.status === "failed" || t.status === "declined" ? "—" : x.share,
        stBg,
        stFg,
      };
    }),

    insight: "MM-04 is quoting 18bps wide on ETH/USDC",

    trackDash: `${(2 * Math.PI * R * 0.75).toFixed(1)} ${(2 * Math.PI * R * 0.25).toFixed(1)}`,
    arcs: (() => {
      const C = 2 * Math.PI * R * 0.75;
      const full = 2 * Math.PI * R;
      const gap = 7;
      let acc = 0;
      return SHARE_SEGS.map((seg, i) => {
        const len = Math.max(0, (seg.pct / 100) * C - gap);
        const arc = {
          color: seg.color,
          dash: `${len.toFixed(1)} ${(full - len).toFixed(1)}`,
          offset: (-acc).toFixed(1),
          op: dimmed(i) ? 1 : 0.3,
        };
        acc += (seg.pct / 100) * C;
        return arc;
      });
    })(),
    donutCap: s.mkTip === null ? "total" : SHARE_SEGS[s.mkTip].label,
    donutVal:
      s.mkTip === null
        ? `$${feesRouted.toLocaleString("en-US")}`
        : `$${Math.round((feesRouted * SHARE_SEGS[s.mkTip].pct) / 100).toLocaleString("en-US")}`,
    tip: s.mkTip !== null,
    tipLabel: s.mkTip === null ? "" : SHARE_SEGS[s.mkTip].label,
    tipPct: s.mkTip === null ? "" : `${SHARE_SEGS[s.mkTip].pct}%`,
    tipAmt:
      s.mkTip === null
        ? ""
        : `$${Math.round((feesRouted * SHARE_SEGS[s.mkTip].pct) / 100).toLocaleString("en-US")} routed`,
    shares: SHARE_SEGS.map((seg, i) => ({
      label: seg.label,
      value: `${seg.pct}%`,
      dot: seg.color,
      op: dimmed(i) ? 1 : 0.4,
    })),

    fills: Math.round(8204 * mult).toLocaleString("en-US"),
    fillsDelta: "↑ 8%",
    bars: scaled.map((v, i) => {
      const on = s.mkBar === i;
      return {
        day: DAY_LABELS[i],
        h: `${Math.max(8, (v / peak) * 100).toFixed(0)}%`,
        bg: on ? "var(--ink)" : i === topIdx ? "var(--lime)" : "#eeeeea",
        dayFg: on || i === topIdx ? "var(--ink)" : "var(--text-dim)",
        tip: on,
        value: `${Math.round(base * (v / (sum / 7))).toLocaleString("en-US")} fills`,
      };
    }),
    avgTop: `${(100 - (avg / peak) * 100).toFixed(1)}%`,
    avgVal: Math.round((8204 * mult) / 7).toLocaleString("en-US"),

    latency: `${Math.round(820 / (0.6 + mult * 0.4))} ms`,
    latDelta: "from 760 ms baseline",
    latLine: LAT_SERIES.map(
      (v, i) => `${i * 70},${(v * (0.8 + mult * 0.2)).toFixed(0)}`,
    ).join(" "),
    latPts: LAT_SERIES.map((v, i) => {
      const y = v * (0.8 + mult * 0.2);
      return {
        left: `${((i / (LAT_SERIES.length - 1)) * 100).toFixed(2)}%`,
        top: `${Math.min(96, Math.max(4, (y / 120) * 100)).toFixed(2)}%`,
        shift: i === 0 ? "-10%" : i === LAT_SERIES.length - 1 ? "-90%" : "-50%",
        tip: s.mkLat === i,
        value: `${Math.round((y / (0.8 + mult * 0.2)) * 8.5)} ms`,
      };
    }),
  };
}
