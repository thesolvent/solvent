import { describe, expect, it } from "vitest";

import { coverage } from "../../src/construction/index";

describe("coverage", () => {
  it("is sufficient below the balance", () => {
    expect(coverage(96n, 100n)).toEqual({ state: "sufficient", pctOfBalance: 96 });
  });

  it("is sufficient at exactly the balance", () => {
    expect(coverage(100n, 100n)).toEqual({ state: "sufficient", pctOfBalance: 100 });
  });

  it("is over above the balance", () => {
    expect(coverage(150n, 100n)).toEqual({ state: "over", pctOfBalance: 150 });
  });

  it("is empty when the balance is zero", () => {
    expect(coverage(50n, 0n)).toEqual({ state: "empty", pctOfBalance: 0 });
  });
});
