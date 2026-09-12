import { expect, it } from "vitest";
import { depthChart } from "./depth-chart";

it("quotes the selected small impact tier without rounding away fractional input", () => {
  const chart = depthChart({
    baseSymbol: "WBTC",
    quoteSymbol: "USDC",
    hoverFrac: 0.001,
    depth: {
      bestPrice: 70_000,
      axisTitle: "WBTC in",
      levels: [
        {
          sizeIn: 0.001,
          output: 69.93,
          price: 69_930,
          impactPct: 0.1,
          makersUsed: 1,
        },
        {
          sizeIn: 1,
          output: 69_300,
          price: 69_300,
          impactPct: 1,
          makersUsed: 1,
        },
      ],
    },
  });
  expect(chart.hover?.size).toBe("0.001000 WBTC");
  expect(chart.hover?.output).toBe("69.9300 USDC");
  expect(chart.hover?.price).toBe("69,930.00 USDC");
  expect(chart.hover?.left).toBe("0.10%");
  expect(chart.xTicks[1].label).toBe("0.25");
});

it("does not invent price extrema between unevenly spaced depth tiers", () => {
  const depth = {
    bestPrice: 10,
    axisTitle: "input",
    levels: [
      { sizeIn: 1, output: 10, price: 10, impactPct: 0, makersUsed: 1 },
      { sizeIn: 2, output: 19, price: 9.5, impactPct: 5, makersUsed: 1 },
      {
        sizeIn: 1000,
        output: 8003,
        price: 8.003,
        impactPct: 20,
        makersUsed: 1,
      },
    ],
  };
  let previousTop = 0;
  for (let step = 0; step <= 200; step++) {
    const chart = depthChart({
      depth,
      baseSymbol: "A",
      quoteSymbol: "B",
      hoverFrac: step / 200,
    });
    const top = Number.parseFloat(chart.hover?.dotTop ?? "NaN");
    expect(top).toBeGreaterThanOrEqual(previousTop - 0.01);
    expect(top).toBeGreaterThanOrEqual(7.5);
    expect(top).toBeLessThanOrEqual(90);
    previousTop = top;
  }
});

it("keeps tiny positive prices visible and ignores non-finite hover coordinates", () => {
  const chart = depthChart({
    baseSymbol: "A",
    quoteSymbol: "B",
    hoverFrac: Number.NaN,
    depth: {
      bestPrice: 1e-10,
      axisTitle: "input",
      levels: [
        {
          sizeIn: 1,
          output: 9e-11,
          price: 9e-11,
          impactPct: 10,
          makersUsed: 1,
        },
      ],
    },
  });
  expect(Number(chart.bestPrice.replaceAll(",", ""))).toBeGreaterThan(0);
  expect(chart.hover).toBeNull();
});

it("does not draw invalid or regressing cumulative samples", () => {
  const point = {
    sizeIn: 10,
    output: 9,
    price: 0.9,
    impactPct: 10,
    makersUsed: 1,
  };
  const options = { baseSymbol: "A", quoteSymbol: "B", hoverFrac: 1 };
  const clean = depthChart({
    ...options,
    depth: { bestPrice: 1, axisTitle: "input", levels: [point] },
  });
  const invalid = depthChart({
    ...options,
    depth: {
      bestPrice: 1,
      axisTitle: "input",
      levels: [
        point,
        { ...point, sizeIn: 5 },
        { ...point, sizeIn: 20, output: 8 },
        { ...point, sizeIn: Number.POSITIVE_INFINITY },
        { ...point, sizeIn: 30, output: Number.NaN },
      ],
    },
  });
  expect(invalid).toEqual(clean);
  const empty = depthChart({ ...options, depth: undefined });
  expect(empty.xTicks).toEqual([]);
  expect(empty.yTicks).toEqual([]);
});

it("keeps geometry finite when decimal scaling approaches the Number boundary", () => {
  const chart = depthChart({
    baseSymbol: "A",
    quoteSymbol: "B",
    hoverFrac: 0.5,
    depth: {
      bestPrice: 1.7975e308,
      axisTitle: "input",
      levels: [
        {
          sizeIn: 1e-232,
          output: 1.7975e76,
          price: 1.7975e308,
          impactPct: 0,
          makersUsed: 1,
        },
      ],
    },
  });
  expect(chart.aggPath).not.toMatch(/NaN|Infinity/);
  expect(chart.hover?.dotTop).not.toMatch(/NaN|Infinity/);
});
