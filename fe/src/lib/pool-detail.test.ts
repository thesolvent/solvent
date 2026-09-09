import { describe, expect, it } from "vitest";

import { poolDetail, type PoolDetailInput } from "./pool-detail";

import type { DepthCurve } from "@/data";

const depth: DepthCurve = {
  axisTitle: "USDC per WETH",
  bestPrice: 1,
  levels: [
    { sizeIn: 100, output: 99, price: 0.99, impactPct: 0.5, makersUsed: 1 },
    { sizeIn: 400, output: 392, price: 0.98, impactPct: 1, makersUsed: 2 },
  ],
};

function detail(over: Partial<PoolDetailInput> = {}) {
  return poolDetail({
    pool: undefined,
    roster: undefined,
    depth,
    hoverFrac: null,
    makerSort: "Virtual",
    ...over,
  });
}

describe("impact stops", () => {
  it("keeps small tiers at their actual fraction rather than a minimum pointer offset", () => {
    expect(detail({ hoverFrac: 0.001 }).hover?.left).toBe("0.10%");
  });

  it("places each tier at its share of the curve", () => {
    expect(detail().impacts).toEqual([
      { label: "0.5%", frac: 0.25 },
      { label: "1.0%", frac: 1 },
    ]);
  });

  it("puts the marker at the hovered tier's size", () => {
    expect(detail({ hoverFrac: 0.25 }).hover?.left).toBe("25.00%");
  });
});

describe("marker placement", () => {
  /** The y the path itself reaches at `x`, read off its own cubics. */
  function pathHeightAt(d: string, x: number): number {
    const nums = (chunk: string) =>
      chunk
        .trim()
        .split(/[\s,]+/)
        .map(Number);
    const [startX, startY] = nums(d.slice(1, d.indexOf("C")));
    let from: [number, number] = [startX, startY];

    for (const chunk of d.split("C").slice(1)) {
      const [c1x, c1y, c2x, c2y, toX, toY] = nums(chunk);
      if (x <= toX) {
        const axis = (
          a: number,
          b: number,
          c: number,
          e: number,
          t: number,
        ) => {
          const s = 1 - t;
          return (
            s * s * s * a +
            3 * s * s * t * b +
            3 * s * t * t * c +
            t * t * t * e
          );
        };
        let lo = 0;
        let hi = 1;
        for (let i = 0; i < 40; i++) {
          const t = (lo + hi) / 2;
          if (axis(from[0], c1x, c2x, toX, t) < x) lo = t;
          else hi = t;
        }
        return axis(from[1], c1y, c2y, toY, (lo + hi) / 2);
      }
      from = [toX, toY];
    }
    return from[1];
  }

  // The dot used to be placed on the straight chord between samples while the curve was drawn
  // as a spline, so it floated off the line wherever the curve turned sharply.
  it("sits on the drawn curve, not the chord between samples", () => {
    for (const frac of [0.1, 0.35, 0.6, 0.85]) {
      const d = detail({ hoverFrac: frac });
      const dotTop = Number.parseFloat(d.hover?.dotTop ?? "0");
      const onPath = (pathHeightAt(d.aggPath, frac * 1000) / 400) * 100;
      expect(dotTop).toBeCloseTo(onPath, 1);
    }
  });
});
