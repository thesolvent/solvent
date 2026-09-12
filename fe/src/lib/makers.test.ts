import { describe, expect, it } from "vitest";

import type { MakerDashboard, Position } from "@/data/makers";
import { INITIAL_STATE, type AppState } from "@/state";

import { LATENCY_VIEWBOX_WIDTH, makerView } from "./makers";

const DAY = 86_400;

function buckets(n: number, latencyMs: number | null = 400) {
  return Array.from({ length: n }, (_, i) => ({
    from: i * DAY,
    to: (i + 1) * DAY,
    fills: i,
    latencyMs,
  }));
}

function dashboard(patch: Partial<MakerDashboard> = {}): MakerDashboard {
  return {
    address: "0xmaker",
    activePositions: 2,
    sharedLiquidityUsd: 1000,
    windowDays: 7,
    volumeUsd: 10,
    feesUsd: 1,
    walletUsd: 100,
    pullableUsd: 90,
    coverage: 0.9,
    liquidityChangePct: null,
    volumeChangePct: null,
    feesChangePct: null,
    fills: 3,
    pairFills: 12,
    sharePct: 25,
    latencyMs: 400,
    previousLatencyMs: 380,
    fillsChangePct: null,
    activity: buckets(7),
    insight: null,
    ...patch,
  };
}

function view(
  state: Partial<AppState>,
  data: Partial<Parameters<typeof makerView>[1]> = {},
) {
  return makerView(
    { ...INITIAL_STATE, ...state },
    {
      address: "0xmaker",
      dashboard: dashboard(),
      positions: [],
      inventory: [],
      settlements: [],
      rebateCount: 0,
      notice: undefined,
      ...data,
    },
  );
}

describe("fill-share donut", () => {
  it("keeps a segment's own index when zero-length arcs are dropped", () => {
    // No own fills: only "Other makers" draws, and hovering it must not report "This maker".
    const mk = view({}, { dashboard: dashboard({ fills: 0, pairFills: 12 }) });

    expect(mk.arcs).toHaveLength(1);
    expect(mk.arcs[0].label).toBe("Other makers");

    const hovered = view(
      { mkTip: mk.arcs[0].index },
      { dashboard: dashboard({ fills: 0, pairFills: 12 }) },
    );
    expect(hovered.tipLabel).toBe("Other makers");
    expect(hovered.tipPct).toBe("100.0%");
  });

  it("names both segments in the chart's accessible label", () => {
    expect(view({}).donutLabel).toBe(
      "Fill share: This maker 25.0%, Other makers 75.0%",
    );
  });
});

describe("activity chart geometry", () => {
  it("spans the whole viewBox whatever the bucket count", () => {
    for (const n of [7, 30, 90]) {
      const mk = view({}, { dashboard: dashboard({ activity: buckets(n) }) });

      expect(mk.latPts).toHaveLength(n);
      expect(mk.latPts[0].left).toBe("0%");
      expect(mk.latPts[n - 1].left).toBe("100%");

      const xs = mk.latLines[0]
        .split(" ")
        .map((point) => Number(point.split(",")[0]));
      expect(xs[0]).toBe(0);
      expect(xs[xs.length - 1]).toBe(LATENCY_VIEWBOX_WIDTH);
    }
  });

  it("centres a lone bucket rather than pinning it to the left edge", () => {
    const mk = view({}, { dashboard: dashboard({ activity: buckets(1) }) });

    expect(mk.latPts[0].left).toBe("50%");
    expect(mk.latLines[0]).toBe("210.0,78.7 210.0,78.7");
  });
});

describe("insight", () => {
  it("is null with no dashboard, so a failed load is never framed as one", () => {
    expect(
      view({}, { dashboard: undefined, notice: "Updates unavailable" }),
    ).toHaveProperty("insight", null);
  });

  it("reports the fill count once the dashboard answers", () => {
    expect(view({}).insight).toBe(
      "This maker filled 3 orders in the last 7 days.",
    );
  });
});

describe("coverage split", () => {
  const position = (patch: Partial<Position>): Position =>
    ({
      hash: "0xpos",
      maker: "0xmaker",
      pair: "WETH/USDC",
      base: { address: "0xb", decimals: 18, symbol: "WETH" },
      quote: { address: "0xq", decimals: 6, symbol: "USDC" },
      curve: "Concentrated",
      feeBps: 30,
      pairType: "Volatile",
      rangeKind: "bounded",
      coverage: 1,
      committedUsd: null,
      committed: [
        { address: "0xb", symbol: "WETH", display: "1", usd: null },
        { address: "0xq", symbol: "USDC", display: "2", usd: null },
      ],
      opening: [],
      feesUsd: null,
      apyPct: null,
      volumeUsd: null,
      ...patch,
    }) as Position;

  it("does not claim a 0% split when neither side is valued", () => {
    const [row] = view({}, { positions: [position({})] }).positions;

    expect(row.splitKnown).toBe(false);
  });

  it("reports the split when the committed value is known", () => {
    const [row] = view(
      {},
      {
        positions: [
          position({
            committedUsd: 100,
            committed: [
              { address: "0xb", symbol: "WETH", display: "1", usd: 40 },
              { address: "0xq", symbol: "USDC", display: "60", usd: 60 },
            ],
          }),
        ],
      },
    ).positions;

    expect(row.splitKnown).toBe(true);
    expect(row.splitA).toBe("40%");
    expect(row.labelA).toBe("40.0% WETH");
  });
});
