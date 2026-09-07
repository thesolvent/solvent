import { ACTIVITY, POOLS, STRATS, TRADES } from "@/data";
import type { AppState, StratSel } from "@/state";

export const STATUS_TONE: Record<string, [string, string]> = {
  confirmed: ["var(--lime-wash-soft)", "var(--green-darkest)"],
  declined: ["var(--warn-bg)", "var(--warn-ink)"],
  "reorg-open": ["var(--info-bg)", "var(--info-ink)"],
  failed: ["var(--err-bg)", "var(--err-ink)"],
  pending: ["#eef0f4", "#4a5a72"],
  partial: ["var(--warn-bg)", "var(--warn-ink)"],
};

const KIND_TONE: Record<string, [string, string]> = {
  pull: ["var(--lime-wash-soft)", "var(--green-darkest)"],
  push: ["var(--info-bg)", "var(--info-ink)"],
  dock: ["var(--surface)", "var(--text-mid)"],
  register: ["#f4f0e2", "#7a6320"],
};

export const DROP_OPTIONS = {
  xpType: ["All types", "pull", "push", "dock", "register"],
  xpEnt: ["All entities", "Maker", "Resolver"],
  xpStatus: ["All status", "confirmed", "declined", "reorg-open", "failed"],
  xpPair: [
    "All pairs",
    "ETH/USDC",
    "WBTC/USDC",
    "USDC/USDT",
    "WETH/USDC",
    "SOL/USDC",
  ],
} as const;

export type DropKey = keyof typeof DROP_OPTIONS;

export function explorerView(s: AppState) {
  const rows = ACTIVITY.filter(
    (r) =>
      (s.xpType === "All types" || r.kind === s.xpType) &&
      (s.xpEnt === "All entities" || r.ent === s.xpEnt.toLowerCase()),
  );
  const trades = TRADES.filter(
    (t) =>
      (s.xpStatus === "All status" || t.status === s.xpStatus) &&
      (s.xpPair === "All pairs" || t.pair === s.xpPair),
  );

  return {
    isTrades: s.xpTab === "Trades",
    pageTitle: s.xpTab === "Trades" ? "Settled trades" : "Protocol activity",
    pageDesc:
      s.xpTab === "Trades"
        ? "Every intent settled through Solvent: pair, in → out, makers sourced, status, price impact and tx."
        : "Aqua-level events: makers registering strategies, pushing and pulling balance, and docking positions.",
    stats: [
      {
        label: "Block height",
        value: "41,310",
        sub: "live",
        accent: "var(--green)",
      },
      {
        label: "Events, 24h",
        value: "18,204",
        sub: "▲ 6.1%",
        accent: "var(--green-deep)",
      },
      {
        label: "Trades settled",
        value: "2,146",
        sub: "97.2% confirmed",
        accent: "var(--text-muted)",
      },
      {
        label: "Median impact",
        value: "0.05%",
        sub: "across 6 pairs",
        accent: "var(--text-muted)",
      },
      {
        label: "Active makers",
        value: "12",
        sub: "3 quoting now",
        accent: "var(--text-muted)",
      },
    ].map((k, i) => ({
      ...k,
      sep: i === 0 ? "transparent" : "var(--line)",
    })),
    rows: rows.map((r) => ({
      row: r,
      hasTrade: !!r.trade,
      kindBg: (KIND_TONE[r.kind] ?? KIND_TONE.dock)[0],
      kindFg: (KIND_TONE[r.kind] ?? KIND_TONE.dock)[1],
      // Rows that name a trade open it; the rest open the maker's strategy.
      target:
        r.link === "trade"
          ? {
              kind: "trade" as const,
              index: Math.max(
                0,
                TRADES.findIndex((t) => `#${t.id}` === r.trade),
              ),
            }
          : {
              kind: "strategy" as const,
              sel: {
                maker: r.who,
                curve: "Concentrated",
                pair: (r.text.match(/[A-Z]+\/[A-Z]+/) ?? ["ETH/USDC"])[0],
              } satisfies StratSel,
            },
    })),
    rowNote: `${rows.length} of ${ACTIVITY.length} events`,
    trades: trades.map((t) => ({
      index: TRADES.indexOf(t),
      blk: t.blk === "—" ? "—" : `blk ${t.blk}`,
      pair: t.pair,
      inn: t.inn,
      out: t.out,
      makers: t.makers ? String(t.makers) : "—",
      impact: t.impact,
      status: t.status,
      tx: t.tx,
      stBg: (STATUS_TONE[t.status] ?? STATUS_TONE.failed)[0],
      stFg: (STATUS_TONE[t.status] ?? STATUS_TONE.failed)[1],
    })),
    tradeNote: `Page 1 / 42 · ${trades.length} shown`,
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
const STAGE_TIMES = ["+0s", "+0.4s", "+0.9s", "+1.2s", "+2.0s", "+6.0s"];

const LEG_DEFS = [
  { hash: "0x3f9a…c210", maker: "MM-01", curve: "XYC", share: 36 },
  { hash: "0x9f3c…4471", maker: "MM-04", curve: "Concentrated", share: 64 },
  { hash: "0x77de…18a2", maker: "MM-07", curve: "Stable", share: 0 },
  { hash: "0xc410…9b31", maker: "MM-11", curve: "XYC", share: 0 },
];

export function tradeDetail(s: AppState) {
  const t = TRADES[s.xpTrade ?? 0] ?? TRADES[0];
  const reached =
    t.status === "confirmed"
      ? 6
      : t.status === "declined"
        ? 2
        : t.status === "failed"
          ? 5
          : 6;
  const outNum = parseFloat(String(t.out).replace(/[^0-9.]/g, "")) || 0;
  const legs = LEG_DEFS.slice(0, Math.max(1, Math.min(4, t.makers + 1)));
  const wsum = legs.reduce((a, l) => a + (l.share || 25), 0);
  const [stBg, stFg] = STATUS_TONE[t.status] ?? STATUS_TONE.failed;

  return {
    trade: t,
    id: `Trade #${t.id}`,
    status: t.status,
    stBg,
    stFg,
    meta: `${t.blk === "—" ? "not settled" : `blk ${t.blk}`} · 6s ago`,
    tx: t.tx,
    summary: [
      { label: "In → out", value: `${t.inn} → ${t.out}` },
      { label: "Price impact", value: t.impact },
      { label: "Resolver", value: "Zero-inventory" },
      { label: "Makers", value: t.makers ? String(t.makers) : "—" },
    ].map((k, i) => ({
      ...k,
      sep: i === 0 ? "transparent" : "var(--line)",
    })),
    headMeta: STAGE_TIMES[reached - 1],
    stageDone: reached,
    phases: [
      {
        tag: "Phase 1",
        name: "Quote & reserve",
        rule: reached >= 3 ? "var(--lime)" : "var(--line)",
      },
      {
        tag: "Phase 2",
        name: "Simulate & settle",
        rule:
          reached >= 6
            ? "var(--ink)"
            : reached > 3
              ? "var(--line-mid)"
              : "var(--line)",
      },
    ],
    steps: STAGES.map((st, i) => {
      const done = i < reached;
      const hot = s.tdStage === i;
      return {
        label: st,
        barX: `calc(${(i * 16.666).toFixed(3)}% + 3px)`,
        barStyle: done ? "solid" : "dashed",
        barBd: done ? (i < 3 ? "var(--lime)" : "var(--ink)") : "#e0e0dc",
        barBg: done ? (i < 3 ? "var(--lime)" : "var(--ink)") : "transparent",
        leadH: `${i % 2 ? 26 : 8}px`,
        state: done ? "" : "pending",
        meta: done ? STAGE_TIMES[i] : "pending",
        scale: hot ? "translateY(-2px)" : "none",
        barShadow: hot && done ? "0 6px 16px rgba(11,11,11,0.16)" : "none",
        delay: done ? `${i * 130}ms` : "0ms",
        fg: done ? "var(--ink)" : "var(--text-dim)",
      };
    }),
    legs: t.makers
      ? legs.map((l) => {
          const share = ((l.share || 25) / wsum) * 100;
          return {
            hash: l.hash,
            maker: l.maker,
            curve: l.curve,
            tag: l.maker.replace("MM-", ""),
            amt: `${((share / 100) * parseFloat(t.inn) || 0).toFixed(2)} ${t.inn.split(" ")[1]} → ${Math.round((outNum * share) / 100).toLocaleString("en-US")} ${t.out.split(" ")[1] ?? ""}`,
            share: `${share.toFixed(0)}%`,
            barW: `${share.toFixed(1)}%`,
            sel: {
              maker: l.maker,
              curve: l.curve,
              pair: t.pair,
            } satisfies StratSel,
          };
        })
      : [],
    facts: [
      { label: "Taker", value: t.taker },
      {
        label: "Order hash",
        value: `0x${t.id}…${t.taker.slice(-4).toLowerCase()}`,
      },
      { label: "Deadline", value: `blk ${t.dl}` },
      {
        label: "Signature",
        value:
          t.status === "declined"
            ? "— declined before signature"
            : `0x${t.id}2f…a71b 1c`,
      },
    ],
    profit: t.spread ? `${t.spread.toFixed(1)} ${t.unit}` : "—",
    profitTag:
      t.status === "confirmed"
        ? "resolver spread"
        : t.status === "reorg-open"
          ? "unsettled — reorg"
          : t.status === "declined"
            ? "not earned — declined"
            : "not earned — failed",
    empty: !t.makers,
  };
}

const MAKER_PROFILES: Record<
  string,
  {
    pfx: string;
    w: number;
    fee: string;
    up: string;
    last: string;
    backed: boolean;
    docked?: boolean;
  }
> = {
  "MM-01": {
    pfx: "0x1a2b",
    w: 0.29,
    fee: "3 bps",
    up: "98.7%",
    last: "34s ago",
    backed: true,
  },
  "MM-04": {
    pfx: "0x9f3c",
    w: 0.51,
    fee: "5 bps",
    up: "99.3%",
    last: "6s ago",
    backed: true,
  },
  "MM-07": {
    pfx: "0x77de",
    w: 0.2,
    fee: "8 bps",
    up: "96.4%",
    last: "3m ago",
    backed: false,
  },
  "MM-11": {
    pfx: "0xc410",
    w: 0.34,
    fee: "6 bps",
    up: "—",
    last: "docked at blk 41,120",
    backed: true,
    docked: true,
  },
};

const PEG_W = { up: 0.29, dn: 0.45 };

export function strategyDetail(sel: StratSel | null) {
  const base = (() => {
    if (!sel) return STRATS[0];
    const pair = String(sel.pair).replace(/\s*\/\s*/, " / ");
    return STRATS.find((x) => x.pair === pair) ?? STRATS[0];
  })();

  // A selector scales the archetype strategy down to that maker's share of it.
  const st = (() => {
    if (!sel) return base;
    const pair = String(sel.pair).replace(/\s*\/\s*/, " / ");
    const curve = sel.curve === "Stable" ? "Pegged" : sel.curve;
    const profile = MAKER_PROFILES[sel.maker] ?? {
      pfx: "0x3e8a",
      w: 0.25,
      fee: "7 bps",
      up: "97.8%",
      last: "1m ago",
      backed: true,
    };

    const units = String(base.virt)
      .split(" · ")
      .map((seg) => {
        const bits = seg.split(" ");
        return {
          n: parseFloat(bits[0].replace(/,/g, "")) || 0,
          sym: bits.slice(1).join(" "),
        };
      });
    const fmtU = (scale: number) =>
      units
        .map(
          (u) =>
            `${(u.n * scale).toLocaleString("en-US", { maximumFractionDigits: u.n * scale < 100 ? 2 : 0 })} ${u.sym}`,
        )
        .join(" · ");

    const isPeg = curve === "Pegged";
    const fillsN = Math.round(
      (parseFloat(String(base.fills).replace(/,/g, "")) || 0) * profile.w,
    );
    const volN =
      (parseFloat(String(base.vol).replace(/[^0-9.]/g, "")) || 0) * profile.w;
    const seedN = (parseInt(String(sel.maker).replace(/\D/g, ""), 10) || 3) + 7;
    const hex = "0123456789abcdef";

    const tx = profile.docked
      ? []
      : base.tx.slice(0, 2 + (seedN % 3)).map((row, i) => {
          const [fromLeg, toLeg] = String(row[1]).split(" → ");
          const scale = profile.w / 0.34;
          const one = fromLeg.split(" ");
          const two = toLeg.split(" ");
          const n1 = (parseFloat(one[0].replace(/,/g, "")) || 0) * scale;
          const n2 = (parseFloat(two[0].replace(/,/g, "")) || 0) * scale;
          const h = (n: number) => hex[(seedN * (i + 2) + n) % 16];
          return [
            `0x${h(1)}${h(5)}…${h(9)}${h(3)}`,
            `${n1.toLocaleString("en-US", { maximumFractionDigits: n1 < 100 ? 2 : 0 })} ${one.slice(1).join(" ")} → ${n2.toLocaleString("en-US", { maximumFractionDigits: n2 < 100 ? 2 : 0 })} ${two.slice(1).join(" ")}`,
            String(
              parseInt(row[2].replace(/,/g, ""), 10) - seedN * (i + 1),
            ).replace(/\B(?=(\d{3})+(?!\d))/g, ","),
            row[3],
          ] as [string, string, string, string];
        });

    return {
      ...base,
      pair,
      curve,
      state: profile.docked ? "docked" : "active",
      tx,
      maker: `${profile.pfx}…${sel.maker}`,
      fee: profile.fee,
      up: profile.up,
      last: profile.last,
      fills: profile.docked ? "0" : fillsN.toLocaleString("en-US"),
      vol: profile.docked ? "$0" : `$${volN.toFixed(volN < 10 ? 2 : 1)}M`,
      virt: fmtU(profile.w),
      act: profile.backed ? fmtU(profile.w) : fmtU(profile.w * 0.78),
      backed: profile.backed,
      short: profile.backed
        ? ""
        : `short ${(units[1].n * profile.w * 0.22).toLocaleString("en-US", { maximumFractionDigits: 0 })} ${units[1].sym}`,
      range:
        curve === "XYC"
          ? "Full range"
          : isPeg
            ? `Peg ${base.ref.toLocaleString("en-US")}`
            : base.range,
      rangeTag:
        curve === "XYC"
          ? "constant product"
          : isPeg
            ? `+${PEG_W.up}% / −${PEG_W.dn}%`
            : base.rangeTag,
      lo:
        curve === "XYC" ? 0 : isPeg ? base.ref * (1 - PEG_W.dn / 100) : base.lo,
      hi:
        curve === "XYC" ? 0 : isPeg ? base.ref * (1 + PEG_W.up / 100) : base.hi,
    };
  })();

  const docked = st.state === "docked";
  const full = st.curve === "XYC";
  const span = st.hi - st.lo || 1;
  const vTop = st.ref * 1.08;
  const vBot = st.ref * 0.92;
  const yOf = (p: number) => 300 - ((p - vBot) / (vTop - vBot)) * 300;

  const N = 44;
  const line = Array.from({ length: N + 1 }, (_, i) => {
    const t = i / N;
    const wob =
      Math.sin(t * 5.3) * 0.42 +
      Math.sin(t * 12.7) * 0.14 +
      Math.sin(t * 2.1 + 1.4) * 0.58;
    const p =
      st.ref +
      wob *
        Math.min(
          st.ref * 0.055,
          full ? st.ref * 0.055 : Math.max(span * 0.7, st.ref * 0.004),
        );
    return `${(t * 640).toFixed(1)},${Math.max(6, Math.min(294, yOf(p))).toFixed(1)}`;
  }).join(" ");

  const cell = (
    label: string,
    value: string,
    tag: string,
    tone: "ok" | "warn" | "n",
  ) => ({
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
  });

  return {
    title: `${st.pair} · ${st.curve}`,
    state: st.state,
    stBg: docked ? "var(--line-faint)" : "var(--lime-wash-soft)",
    stFg: docked ? "var(--text-mid)" : "var(--green-darkest)",
    maker: st.maker,
    pool: st.pair,
    poolIndex: Math.max(
      0,
      POOLS.findIndex(
        (p) => p.pair.replace(/\s/g, "") === st.pair.replace(/\s/g, ""),
      ),
    ),
    since: docked ? "docked at blk 41,120" : `since blk ${st.since}`,
    mid: st.mid,
    bandNote: full
      ? "full range — no bounds"
      : st.curve === "Pegged"
        ? "peg-relative bounds"
        : "bounded band",
    bandY: full ? 0 : Number(yOf(st.hi).toFixed(1)),
    bandH: full ? 300 : Number(Math.max(4, yOf(st.lo) - yOf(st.hi)).toFixed(1)),
    bandY2: full ? 300 : Number(yOf(st.lo).toFixed(1)),
    line,
    axisFrom: "7d ago",
    axisTo: "now",
    tickLo: (st.ref * 0.92).toLocaleString("en-US", {
      maximumFractionDigits: st.ref < 10 ? 4 : 0,
    }),
    tickHi: (st.ref * 1.08).toLocaleString("en-US", {
      maximumFractionDigits: st.ref < 10 ? 4 : 0,
    }),
    spark: Array.from({ length: 28 }, (_, i) => {
      const v = docked ? 4 : Math.abs(Math.sin(i * 1.7 + 0.6)) * 74 + 12;
      return {
        h: `${Math.max(3, v).toFixed(0)}%`,
        bg: docked ? "var(--surface)" : i > 24 ? "var(--lime)" : "#e2e2de",
      };
    }),
    shape: [
      cell("Virtual balance", st.virt, "committed", "n"),
      cell(
        "Actual / pullable",
        st.act,
        st.backed ? "fully backed ✓" : st.short,
        st.backed ? "ok" : "warn",
      ),
      cell("Fee", st.fee, "protocol fee tier", "n"),
      cell("Range", st.range, st.rangeTag, "n"),
    ].map((k, i) => ({
      ...k,
      sep: i === 0 ? "transparent" : "var(--line)",
    })),
    active: [
      {
        label: "Fills (7d)",
        value: st.fills,
        sub: docked ? "docked" : "pulls",
      },
      { label: "Volume (7d)", value: st.vol, sub: "routed" },
      {
        label: "Quote uptime",
        value: st.up,
        sub: docked ? "not quoting" : "7d",
      },
      {
        label: "Last fill",
        value: docked ? "—" : st.last,
        sub: docked ? st.last : "",
      },
    ].map((k, i) => ({
      ...k,
      sep: i === 0 ? "transparent" : "var(--line)",
    })),
    fills: st.tx.map((x) => ({
      hash: x[0],
      flow: x[1],
      blk: x[2],
      trade: `trade ${x[3]}`,
    })),
    noFills: !st.tx.length,
  };
}
