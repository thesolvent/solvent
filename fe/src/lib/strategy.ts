import { POOLS, STRATS } from "@/data";
import type { StratSel } from "@/state";

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
