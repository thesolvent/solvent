import { expect, it } from "vitest";
import { strategyChart } from "./strategy";

it("steps only at recorded price changes and breaks the line when a strategy cannot quote", () => {
  const chart = strategyChart(undefined, {
    from: 0,
    to: 100,
    createdBlock: 1,
    prices: [
      { at: 10, price: 1 },
      { at: 20, price: 2 },
      { at: 30, price: null },
      { at: 50, price: 3 },
    ],
  });
  const paths = chart.lines.map((line) =>
    line.split(" ").map((point) => point.split(",").map(Number)),
  );
  expect(paths.map((path) => path.map((point) => point[0]))).toEqual([
    [64, 128, 128, 192],
    [320, 640],
  ]);
  expect(paths[0][0][1]).toBe(paths[0][1][1]);
  expect(paths[0][2][1]).toBe(paths[0][3][1]);
  expect(strategyChart(undefined, undefined).lines).toEqual([]);
});
