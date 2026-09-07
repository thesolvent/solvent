import { POOLS, TOKENS, type Pool } from "@/data";

/** Per-maker ladder colours, cycled by roster index. */
const HUES = [
  "#63c31c",
  "#3b82c4",
  "#a855c7",
  "#e08a2e",
  "#14b8a6",
  "#d4457a",
  "#7c8f2f",
  "#5b6bd6",
];

/** `slack` is the fraction of virtual size the maker is actually quoting. */
const ROSTER = [
  {
    addr: "0x1a2b…MM-01",
    curve: "XYC",
    w: 6.1,
    bps: "5 bps",
    up: "99.8%",
    slack: 0.88,
  },
  {
    addr: "0x9f3c…MM-04",
    curve: "Concentrated",
    w: 4.4,
    bps: "5 bps",
    up: "99.4%",
    slack: 1,
  },
  {
    addr: "0x77de…MM-07",
    curve: "Stable",
    w: 3.8,
    bps: "4 bps",
    up: "97.1%",
    slack: 0.76,
  },
  {
    addr: "0xc410…MM-11",
    curve: "XYC",
    w: 2.6,
    bps: "6 bps",
    up: "99.9%",
    slack: 1,
  },
  {
    addr: "0x3e8a…MM-02",
    curve: "Concentrated",
    w: 2.1,
    bps: "7 bps",
    up: "98.6%",
    slack: 0.91,
  },
  {
    addr: "0xb56f…MM-09",
    curve: "Stable",
    w: 1.7,
    bps: "8 bps",
    up: "99.1%",
    slack: 1,
  },
  {
    addr: "0x04cd…MM-15",
    curve: "XYC",
    w: 1.3,
    bps: "9 bps",
    up: "96.4%",
    slack: 0.82,
  },
  {
    addr: "0xd7e2…MM-18",
    curve: "Concentrated",
    w: 1.0,
    bps: "11 bps",
    up: "99.5%",
    slack: 1,
  },
];

const FILLS = [
  {
    hash: "0x8f…a1",
    kind: "pull",
    from: "1.2 ETH",
    to: "3,720 USDC",
    size: 3720,
    blk: "41,203",
    ago: "12s",
  },
  {
    hash: "0x2b…c9",
    kind: "pull",
    from: "0.4 ETH",
    to: "1,240 USDC",
    size: 1240,
    blk: "41,198",
    ago: "48s",
  },
  {
    hash: "0x5d…7e",
    kind: "push",
    from: "9,800 USDC",
    to: "3.16 ETH",
    size: 9800,
    blk: "41,190",
    ago: "2m",
  },
  {
    hash: "0xa3…04",
    kind: "pull",
    from: "0.9 ETH",
    to: "2,790 USDC",
    size: 2790,
    blk: "41,181",
    ago: "4m",
  },
  {
    hash: "0xe1…bb",
    kind: "pull",
    from: "2.4 ETH",
    to: "7,440 USDC",
    size: 7440,
    blk: "41,172",
    ago: "6m",
  },
];

export type DetailMaker = {
  addr: string;
  curve: string;
  up: string;
  act: string;
  /** Teal when the maker quotes under its virtual size, lime when at full size. */
  gap: string;
  stateBg: string;
  stateFg: string;
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
  pool: Pool;
  pair: string;
  venue: string;
  fee: string;
  apr: string;
  tvl: string;
  vol: string;
  fills: string;
  spread: string;
  aggPath: string;
  yTicks: { label: string; top: string }[];
  xTicks: { label: string; left: string }[];
  hover: DetailHover | null;
  axisTitle: string;
  priceTitle: string;
  bestPrice: string;
  imp1Label: string;
  imp1: string;
  imp1Size: string;
  imp5Label: string;
  imp5: string;
  imp5Size: string;
  totalLiq: string;
  makers: DetailMaker[];
  makerTotal: number;
  settlements: { from: string; to: string; ago: string }[];
};

/**
 * Catmull-Rom-ish smoothing through segment midpoints, anchored at the first
 * segment's start and the last one's end.
 */
function smoothPath(
  segs: { from: number; to: number; price: number }[],
  X: (n: number) => number,
  Y: (n: number) => number,
) {
  if (!segs.length) return "";
  const pts = segs.map((sg) => [X((sg.from + sg.to) / 2), Y(sg.price)]);
  pts.unshift([X(segs[0].from), Y(segs[0].price)]);
  pts.push([X(segs[segs.length - 1].to), Y(segs[segs.length - 1].price)]);

  let d = `M ${pts[0][0].toFixed(1)} ${pts[0][1].toFixed(1)}`;
  for (let i = 0; i < pts.length - 1; i++) {
    const p0 = pts[Math.max(0, i - 1)];
    const p1 = pts[i];
    const p2 = pts[i + 1];
    const p3 = pts[Math.min(pts.length - 1, i + 2)];
    const c1x = p1[0] + (p2[0] - p0[0]) / 6;
    const c1y = p1[1] + (p2[1] - p0[1]) / 6;
    const c2x = p2[0] - (p3[0] - p1[0]) / 6;
    const c2y = p2[1] - (p3[1] - p1[1]) / 6;
    d += ` C ${c1x.toFixed(1)} ${c1y.toFixed(1)}, ${c2x.toFixed(1)} ${c2y.toFixed(1)}, ${p2[0].toFixed(1)} ${p2[1].toFixed(1)}`;
  }
  return d;
}

export function poolDetail(
  detailIndex: number | null,
  detailRange: string,
  makerSort: string,
  hoverFrac: number | null,
): PoolDetail {
  const p = POOLS[detailIndex ?? 0];
  const makerCount = parseInt(
    (p.venue.match(/(\d+)\s*makers/) ?? ["", "4"])[1],
    10,
  );
  const poolTvl = parseFloat(p.tvl.replace(/[^0-9.]/g, ""));

  const roster = ROSTER.slice(0, makerCount);
  const wSum = roster.reduce((a, m) => a + m.w, 0);
  const makers = roster.map((m) => {
    const virt = poolTvl * (m.w / wSum);
    return {
      addr: m.addr,
      curve: m.curve,
      up: m.up,
      virt,
      act: virt * m.slack,
    };
  });

  const byVirt = makerSort === "Virtual";
  const sortedMakers = [...makers].sort((a, b) =>
    byVirt ? b.virt - a.virt : b.act - a.act,
  );

  const seed = (detailIndex ?? 0) + (detailRange === "30d" ? 7 : 0);
  const baseSym = (p.pair.split("/")[0] || "ETH").trim();
  const quoteSym = (p.pair.split("/")[1] || "USDC").trim();
  const mid = TOKENS.find((t) => t.symbol === baseSym)?.price ?? 3000;
  const depthRange =
    detailRange === "30d"
      ? 0.026
      : detailRange === "All"
        ? 0.034
        : detailRange === "24h"
          ? 0.012
          : 0.018;

  const ladders = makers.map((m, k) => {
    const steps = 6;
    const unit = ((m.act / mid) * 1e6) / steps;
    const start = 0.0006 + (((k * 37 + seed * 11) % 19) / 19) * 0.004;
    return {
      color: HUES[k % HUES.length],
      levels: Array.from({ length: steps }, (_, j) => ({
        price:
          mid *
          (1 +
            start +
            (j / (steps - 1)) *
              depthRange *
              (0.8 + ((k * 13 + j * 7) % 9) / 22)),
        size: unit * (1 + ((k * 5 + j * 3) % 7) / 10),
        maker: k,
      })),
    };
  });

  const all = ladders
    .flatMap((l) => l.levels)
    .sort((x, y) => x.price - y.price);
  let cum = 0;
  const agg = all.map((lv) => {
    const from = cum;
    cum += lv.size;
    return { from, to: cum, price: lv.price, maker: lv.maker };
  });

  const totalSize = cum || 1;
  const pMin = mid * 0.999;
  const pMax = agg.length ? agg[agg.length - 1].price * 1.0012 : mid * 1.03;
  const X = (sz: number) => (sz / totalSize) * 1000;
  const Y = (pr: number) => 360 - ((pr - pMin) / (pMax - pMin)) * 330;

  const yTicks = Array.from({ length: 5 }, (_, i) => {
    const pr = pMin + (pMax - pMin) * (i / 4);
    return {
      label: pr.toLocaleString("en-US", { maximumFractionDigits: 0 }),
      top: `${((Y(pr) / 400) * 100).toFixed(2)}%`,
    };
  });
  const xTicks = Array.from({ length: 5 }, (_, i) => ({
    label: Math.round(totalSize * (i / 4)).toLocaleString("en-US"),
    left: `${((i / 4) * 100).toFixed(1)}%`,
  }));

  // Walk the aggregated ladder to the hovered size, accumulating spend so the
  // tooltip can report the blended effective price rather than the marginal one.
  let hover: DetailHover | null = null;
  if (hoverFrac !== null && hoverFrac !== undefined) {
    const target = Math.max(totalSize * 0.02, hoverFrac * totalSize);
    let spent = 0;
    let filled = 0;
    let last = agg[0]?.price ?? mid;
    const used: Record<number, 1> = {};
    for (const level of agg) {
      const take = Math.min(level.to, target) - level.from;
      if (take <= 0) break;
      spent += take * level.price;
      filled += take;
      last = level.price;
      used[level.maker] = 1;
      if (level.to >= target) break;
    }
    const eff = filled ? spent / filled : last;
    hover = {
      left: `${((target / totalSize) * 100).toFixed(2)}%`,
      dotTop: `${((Y(agg.length ? agg[0].price + (last - agg[0].price) : last) / 400) * 100).toFixed(2)}%`,
      shift: target / totalSize > 0.56 ? "translateX(-108%)" : "translateX(8%)",
      size: `${Math.round(filled).toLocaleString("en-US")} ${baseSym}`,
      price: `${eff.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })} ${quoteSym}`,
      output: `${spent.toLocaleString("en-US", { maximumFractionDigits: 0 })} ${quoteSym}`,
      makers: String(Object.keys(used).length),
      dots: Object.keys(used).map((k) => ({
        bg: HUES[Number(k) % HUES.length],
      })),
    };
  }

  const impactAt = (pct: number) => {
    const cap = mid * (1 + pct);
    let sz = 0;
    let spent = 0;
    for (const level of agg) {
      if (level.price > cap) break;
      sz = level.to;
      spent += (level.to - level.from) * level.price;
    }
    return { price: sz ? spent / sz : mid, size: sz };
  };
  const imp1 = impactAt(depthRange * 0.34);
  const imp5 = impactAt(depthRange * 0.72);
  const fmtP = (n: number) =>
    `~${n.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;

  return {
    pool: p,
    pair: p.pair,
    venue: p.venue.split(" · ")[0],
    fee: p.fee,
    apr: p.apr,
    tvl: p.tvl,
    vol: p.vol,
    fills: p.fills,
    spread: (p.range || "").replace(" spread", "") || "—",
    aggPath: smoothPath(agg, X, Y),
    yTicks,
    xTicks,
    hover,
    axisTitle: `Cumulative ${baseSym} available`,
    priceTitle: `${quoteSym} per ${baseSym}`,
    bestPrice: mid.toLocaleString("en-US", {
      minimumFractionDigits: 2,
      maximumFractionDigits: 2,
    }),
    imp1Label: `${(depthRange * 34).toFixed(1)}% impact`,
    imp1: fmtP(imp1.price),
    imp1Size: `~${Math.round(imp1.size).toLocaleString("en-US")} ${baseSym}`,
    imp5Label: `${(depthRange * 72).toFixed(1)}% impact`,
    imp5: fmtP(imp5.price),
    imp5Size: `~${Math.round(imp5.size).toLocaleString("en-US")} ${baseSym}`,
    totalLiq: `${Math.round(totalSize).toLocaleString("en-US")} ${baseSym}`,
    makers: sortedMakers.map((m) => {
      const under = m.act < m.virt;
      return {
        addr: m.addr,
        curve: m.curve,
        up: m.up,
        act: `$${m.act.toFixed(1)}M`,
        gap: under ? "var(--ok-ink)" : "var(--green)",
        stateBg: under ? "var(--ok-bg)" : "var(--lime-wash-soft)",
        stateFg: under ? "var(--ok-ink-deep)" : "var(--green-darkest)",
      };
    }),
    makerTotal: makers.length,
    settlements: FILLS.map((x) => ({ from: x.from, to: x.to, ago: x.ago })),
  };
}
