import type {
  PoolDepth as ApiDepth,
  PoolDetail as ApiDetail,
} from "@solvent/sdk/client";
import { describe, expect, it } from "vitest";

import detailFixture from "@/data/fixtures/pool-detail.json";
import depthFixture from "@/data/fixtures/pool-depth.json";

import { toDepthCurve, toPoolRoster } from "./pool-detail";

/** Captured from the devnet server, so a served-shape change breaks these rather than a view. */
const detail = detailFixture as ApiDetail;
const depth = depthFixture as ApiDepth;
const pair = {
  base: detail.base.address,
  quote: detail.quote.address,
  baseDecimals: detail.base.decimals,
  quoteDecimals: detail.quote.decimals,
};

describe("toPoolRoster", () => {
  it("carries each maker's curve, fee and committed value", () => {
    const roster = toPoolRoster(detail);
    const maker = roster.makers[0];

    expect(roster.pool.pair).toBe("DAI / USDC");
    expect(maker.curve).toBe("Pegged");
    expect(maker.virtualUsd).toBeGreaterThan(0);
    expect(maker.balances.map((b) => b.symbol)).toContain("USDC");
  });

  it("orders makers by committed value, largest first", () => {
    const small = {
      ...detail.makers[0],
      virtual: { entries: [], total_usd: 1 },
    };
    const roster = toPoolRoster({
      ...detail,
      makers: [small, detail.makers[0]],
    });

    expect(roster.makers[0].virtualUsd).toBeGreaterThan(
      roster.makers[1].virtualUsd!,
    );
  });
});

describe("toDepthCurve", () => {
  it("scales each side by its own token's decimals", () => {
    const curve = toDepthCurve(depth, pair);
    const level = curve.levels[0];
    const raw = depth.points[0];

    expect(level.sizeIn).toBeCloseTo(
      Number(raw.trade_size) / 10 ** pair.baseDecimals,
    );
    expect(level.output).toBeCloseTo(
      Number(raw.output) / 10 ** pair.quoteDecimals,
    );
    // Effective price is output over input, so the two scalings must agree.
    expect(level.output / level.sizeIn).toBeCloseTo(level.price, 3);
  });

  it("keeps the server's impact tiers and reports the tip price", () => {
    const curve = toDepthCurve(depth, pair);

    expect(curve.levels.map((l) => l.impactPct)).toEqual(
      depth.points.map((p) => p.impact_pct),
    );
    expect(curve.bestPrice).toBeCloseTo(Number.parseFloat(depth.best_price));
  });
});
