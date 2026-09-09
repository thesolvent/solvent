import { describe, expect, it } from "vitest";

import type { CreatePair } from "@/ports/positions";
import { INITIAL_STATE } from "@/state";
import { clampBand, createPosition, formatPrice } from "./create-position";

const pair = {
  base: {
    address: "0x2222222222222222222222222222222222222222",
    decimals: 6,
    symbol: "USDC",
    name: "USD Coin",
    tags: ["USD"],
    balance: 100,
    balanceRaw: 100_000_000n,
    valueUsd: 100,
    changePct: 0,
    tint: "#eee",
  },
  quote: {
    address: "0x3333333333333333333333333333333333333333",
    decimals: 18,
    symbol: "WETH",
    name: "Wrapped Ether",
    tags: ["ETH"],
    balance: 100,
    balanceRaw: 100_000_000_000_000_000_000n,
    valueUsd: 250_000,
    changePct: 0,
    tint: "#eef",
  },
  mid: 2_500,
  tvlUsd: 500_000,
  type: "Volatile",
  defaultBandPct: 5,
  defaultFeeBps: 5,
} satisfies CreatePair;

describe("create position amounts", () => {
  it.each(["1 USDC", "1e2", "0.0000001"])(
    "keeps an invalid token amount %s out of the wallet flow",
    (amountBase) => {
      const view = createPosition(
        {
          ...INITIAL_STATE,
          corePair: 0,
          strategy: "Concentrated",
          amtA: amountBase,
          amtB: "1",
        },
        [pair],
      );

      expect(view.ctaDisabled).toBe(true);
      expect(view.cta).toBe("Enter a valid deposit amount");
    },
  );

  it("uses the curve allocator for pegged amounts and ignores chart range width", () => {
    const stablePair = {
      ...pair,
      base: {
        ...pair.base,
        decimals: 6,
        symbol: "USDT",
        balance: 99.49,
        balanceRaw: 99_490_000n,
      },
      quote: {
        ...pair.quote,
        decimals: 6,
        symbol: "USDC",
        balance: 10_096.73,
        balanceRaw: 10_096_730_000n,
      },
      mid: 0.9992,
      type: "Stable" as const,
    };
    const narrow = createPosition(
      {
        ...INITIAL_STATE,
        strategy: "Pegged",
        bandMin: -0.01,
        bandMax: 0.01,
      },
      [stablePair],
    );
    const wide = createPosition(
      {
        ...INITIAL_STATE,
        strategy: "Pegged",
        bandMin: -5,
        bandMax: 5,
      },
      [stablePair],
    );

    expect(narrow.amountsFromA("99.49")).toEqual({
      amtA: "99.49",
      amtB: "99.410408",
    });
    expect(narrow.amountsFromB("99.410408")).toEqual({
      amtA: "99.49",
      amtB: "99.410408",
    });
    expect(narrow.maxAmounts).toEqual({
      amtA: "99.49",
      amtB: "99.410408",
    });
    expect(narrow.bPerA).toBe(wide.bPerA);
    expect(narrow.pairNote).toBe(
      "1 USDT pairs with 0.9992 USDC at the live market price",
    );
  });

  it("does not allow stale amounts from another curve to be submitted", () => {
    const view = createPosition(
      {
        ...INITIAL_STATE,
        step: 4,
        strategy: "Full range",
        amtA: "1",
        amtB: "10",
      },
      [pair],
    );

    expect(view.ctaDisabled).toBe(true);
    expect(view.cta).toBe("Recalculate deposit amounts");
    expect(view.footNote).toBe(
      "Deposit amounts must match the selected curve at the live market price.",
    );
  });

  it.each(["Full range", "Concentrated", "Pegged"] as const)(
    "keeps %s sizing and the final submission invariant on the same SDK allocation",
    (strategy) => {
      const state = {
        ...INITIAL_STATE,
        corePair: 0,
        strategy,
        pegSym: strategy === "Pegged",
        bandMin: -50,
        bandMax: 50,
        amtA: "",
        amtB: "",
      };
      const sizing = createPosition(state, [pair]);
      const amounts = sizing.amountsFromA("0.01");
      const review = createPosition(
        {
          ...state,
          ...amounts,
          step: 4,
        },
        [pair],
      );

      expect(amounts.amtA).toBe("0.01");
      expect(Number(amounts.amtB)).toBeGreaterThan(0);
      expect(review.spotPrice).toBe("2500");
      expect(review.ctaDisabled).toBe(false);
    },
  );
});

describe("pair recommendations", () => {
  it("keeps the full catalog while recommending the six highest-TVL pairs", () => {
    const tvls = [10, 80, 30, 90, 70, 20, 60, 40];
    const pairs = tvls.map((tvlUsd, index) => ({
      ...pair,
      base: { ...pair.base, symbol: `TOKEN${index}` },
      tvlUsd,
    }));

    const view = createPosition(INITIAL_STATE, pairs);

    expect(view.pairs).toHaveLength(8);
    expect(
      view.recommendations.map(({ catalogIndex }) => catalogIndex),
    ).toEqual([3, 1, 4, 6, 7, 2]);
  });
});

describe("custom position fee", () => {
  it("converts a percentage to whole basis points and rejects unsupported values", () => {
    const validState = {
      ...INITIAL_STATE,
      createFee: "Custom",
      customFeePct: "0.17",
    };
    const valid = createPosition(validState, [pair]);

    expect(valid.feeBps).toBe(17);
    expect(valid.feeLabel).toBe("0.17% custom");
    expect(valid.feeProblem).toBeUndefined();

    for (const customFeePct of ["0.001", "100", "100.01", "fee"]) {
      const invalid = createPosition({ ...validState, customFeePct }, [pair]);
      expect(invalid.feeProblem).toBe(
        "Enter a fee from 0% to 99.99% with up to two decimal places.",
      );
      expect(invalid.ctaDisabled).toBe(true);
    }
  });
});

describe("exact wallet limits", () => {
  it("uses raw base units for Max above JavaScript's safe integer range", () => {
    const balanceRaw = 9_007_199_254_740_993_123_456_789n;
    const view = createPosition(INITIAL_STATE, [
      {
        ...pair,
        base: { ...pair.base, balance: 9_007_199_254_740_993_000, balanceRaw },
      },
    ]);

    expect(view.walletA).toBe("9007199254740993123.456789");
    expect(view.maxFromA.amtA).toBe("9007199254740993123.456789");
  });
});

describe("custom position bounds", () => {
  it("caps independently adjusted bounds at 50 percent", () => {
    expect(
      clampBand(
        {
          ...INITIAL_STATE,
          strategy: "Concentrated",
          bandMin: -10,
          bandMax: 10,
        },
        { bandMin: -75, bandMax: 75 },
      ),
    ).toEqual({ bandMin: -50, bandMax: 50 });
  });

  it("caps a symmetric pegged bound at 50 percent", () => {
    expect(
      clampBand(
        {
          ...INITIAL_STATE,
          strategy: "Pegged",
          pegSym: true,
          bandMin: -10,
          bandMax: 10,
        },
        { bandMax: 75 },
      ),
    ).toEqual({ bandMin: -50, bandMax: 50 });
  });

  it("keeps symmetric pegged bounds inside the SwapVM linear-width domain", () => {
    expect(
      clampBand(
        {
          ...INITIAL_STATE,
          strategy: "Pegged",
          pegSym: true,
          bandMin: -0.01,
          bandMax: 0.01,
        },
        { bandMax: 0.004 },
      ),
    ).toEqual({ bandMin: -0.01, bandMax: 0.01 });
  });
});

describe("market history", () => {
  const history = [
    { timestampMs: Date.UTC(2026, 8, 1), price: 2_400, volumeUsd: 1_000 },
    { timestampMs: Date.UTC(2026, 8, 8), price: 2_600, volumeUsd: 2_000 },
  ];

  it("keeps the live midpoint as the strategy anchor for every chart period", () => {
    const week = createPosition(
      { ...INITIAL_STATE, createSpan: "7d", bandMin: -5, bandMax: 5 },
      [pair],
      undefined,
      history,
    );
    const all = createPosition(
      { ...INITIAL_STATE, createSpan: "All", bandMin: -5, bandMax: 5 },
      [pair],
      undefined,
      [
        { timestampMs: Date.UTC(2024, 0, 1), price: 1_000, volumeUsd: 50 },
        { timestampMs: Date.UTC(2026, 8, 8), price: 9_000, volumeUsd: 80 },
      ],
    );

    expect(week.priceMin).toBe("2375");
    expect(week.priceMax).toBe("2625");
    expect(all.priceMin).toBe(week.priceMin);
    expect(all.priceMax).toBe(week.priceMax);
    expect(all.market).toBe(all.opening);
  });

  it("uses real timestamps and volumes and has no generated fallback series", () => {
    const live = createPosition(
      { ...INITIAL_STATE, createSpan: "7d", chartHover: { x: 100, y: 50 } },
      [pair],
      undefined,
      history,
    );
    const unavailable = createPosition(INITIAL_STATE, [pair]);

    expect(live.series).not.toBe("");
    expect(live.vols.map(({ vol }) => vol)).toEqual(["$1K", "$2K"]);
    expect(live.cross?.date).toBe("Sep 8, 2026");
    expect(unavailable.series).toBe("");
    expect(unavailable.vols).toEqual([]);
  });

  it("keeps every price-axis label positive across all-time reversed history", () => {
    const view = createPosition(
      { ...INITIAL_STATE, createSpan: "All", flipped: true },
      [pair],
      undefined,
      [
        { timestampMs: Date.UTC(2017, 7, 1), price: 10, volumeUsd: 50 },
        { timestampMs: Date.UTC(2026, 8, 8), price: 2_500, volumeUsd: 80 },
      ],
    );

    expect(view.axisLo).not.toMatch(/^-/);
    expect(view.series).not.toBe("");
  });
});

describe("price formatting", () => {
  it("shows five significant digits for tiny reversed prices", () => {
    expect(formatPrice(0.000012549070788367136)).toBe("1.2549×10⁻⁵");
    expect(formatPrice(0.9992)).toBe("0.9992");
    expect(formatPrice(79_427.72)).toBe("79,427.72");
  });
});
