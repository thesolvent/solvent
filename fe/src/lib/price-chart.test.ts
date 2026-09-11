import { describe, expect, it } from "vitest";

import type { PairPricePoint } from "@/ports/positions";

import { priceChart } from "./price-chart";

function series(prices: number[]): PairPricePoint[] {
  return prices.map((price, i) => ({
    timestampMs: 1_700_000_000_000 + i * 3_600_000,
    price,
    volumeUsd: undefined,
  }));
}

describe("priceChart", () => {
  it("reports the move across the window, signed", () => {
    const up = priceChart(series([10, 12]), "LINK/USDT");
    expect(up.change).toBe("↗ 20.00%");
    expect(up.changeUp).toBe(true);

    const down = priceChart(series([10, 8]), "LINK/USDT");
    expect(down.change).toBe("↘ 20.00%");
    expect(down.changeUp).toBe(false);
  });

  // A pegged pair has no range to scale against; dividing by it would put every point at
  // infinity and draw nothing.
  it("plots a flat series through the middle instead of dividing by a zero range", () => {
    const flat = priceChart(series([1, 1, 1, 1]), "DAI/USDC");

    expect(flat.linePath).not.toBe("");
    expect(flat.linePath).not.toContain("NaN");
    expect(flat.linePath).not.toContain("Infinity");
    expect(flat.change).toBe("↗ 0.00%");
  });

  it("draws nothing for a period with too few points to form a line", () => {
    const none = priceChart([], "LINK/USDT");
    expect(none.linePath).toBe("");
    expect(none.last).toBeNull();
    expect(none.summary).toBe("No price history for LINK/USDT in this period");

    expect(priceChart(series([5]), "LINK/USDT").linePath).toBe("");
  });

  it("ignores points the source could not price", () => {
    const dirty: PairPricePoint[] = [
      { timestampMs: 1, price: Number.NaN, volumeUsd: undefined },
      ...series([10, 11]),
      { timestampMs: 9, price: 0, volumeUsd: undefined },
    ];

    const model = priceChart(dirty, "LINK/USDT");

    expect(model.linePath).not.toContain("NaN");
    expect(model.change).toBe("↗ 10.00%");
  });

  it("describes the window for a reader who cannot see the plot", () => {
    const model = priceChart(series([10, 15, 12]), "LINK/USDT");

    expect(model.summary).toContain("LINK/USDT moved up 20.00%");
    expect(model.summary).toContain("low 10.00");
    expect(model.summary).toContain("high 15.00");
  });
});
