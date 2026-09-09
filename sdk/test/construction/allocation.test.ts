import { Address as SdkAddress, instructions } from "@1inch/swap-vm-sdk";
import { formatUnits, parseUnits } from "viem";
import { describe, expect, it } from "vitest";

import {
  linearWidthFromSymmetricRangePercent,
  strategyAllocator,
} from "../../src/construction/index";
import type { Address } from "../../src/index";

const BASE = "0x2222222222222222222222222222222222222222" as Address;
const QUOTE = "0x3333333333333333333333333333333333333333" as Address;

describe("strategyAllocator", () => {
  it("centres a mixed-decimal pegged position on the requested spot price", () => {
    const base = { address: BASE, decimals: 6 };
    const quote = { address: QUOTE, decimals: 18 };
    const width = linearWidthFromSymmetricRangePercent(0.1);
    const allocator = strategyAllocator({
      base,
      quote,
      spotPrice: "0.9992",
      curve: { kind: "pegged" },
    });

    const allocation = allocator.fromBase(parseUnits("99.49", base.decimals));
    const price = instructions.peggedSwap.PeggedPrice.fromReserves({
      reserveA: {
        ...sdkToken(base),
        initialReserve: allocation.base,
        currentReserve: allocation.base,
      },
      reserveB: {
        ...sdkToken(quote),
        initialReserve: allocation.quote,
        currentReserve: allocation.quote,
      },
      linearWidth: width,
    });

    expect(price.toHuman(new SdkAddress(quote.address))).toBe("0.9992");
  });

  it("keeps pegged allocation independent of the selected band width", () => {
    const input = {
      base: { address: BASE, decimals: 6 },
      quote: { address: QUOTE, decimals: 6 },
      spotPrice: "0.9992",
      curve: { kind: "pegged" as const },
    };
    const fixed = parseUnits("99.49", input.base.decimals);

    expect(strategyAllocator(input).fromBase(fixed)).toEqual({
      base: fixed,
      quote: parseUnits("99.410408", input.quote.decimals),
    });
  });

  it("centres concentrated reserves on the requested spot within its bounds", () => {
    const base = { address: QUOTE, decimals: 18 };
    const quote = { address: BASE, decimals: 6 };
    const bounds = { priceMin: "2375", priceMax: "2625" };
    const allocator = strategyAllocator({
      base,
      quote,
      spotPrice: "2500",
      curve: { kind: "concentrated", ...bounds },
    });
    const allocation = allocator.fromBase(parseUnits("1", base.decimals));
    const pair = pricePair(base, quote);
    const recovered = instructions.concentrate.PriceRange.fromPriceBounds(
      {
        minPrice: instructions.concentrate.Price.fromHuman(
          bounds.priceMin,
          pair,
        ),
        maxPrice: instructions.concentrate.Price.fromHuman(
          bounds.priceMax,
          pair,
        ),
      },
      {
        reserveA: instructions.concentrate.TokenReserve.new({
          token: new SdkAddress(base.address),
          reserve: allocation.base,
        }),
        reserveB: instructions.concentrate.TokenReserve.new({
          token: new SdkAddress(quote.address),
          reserve: allocation.quote,
        }),
      },
    );

    expect(recovered.spotPrice.toHuman(new SdkAddress(quote.address))).toBe(
      "2500",
    );
  });

  it("uses the requested spot ratio for full-range reserves in both directions", () => {
    const base = { address: BASE, decimals: 6 };
    const quote = { address: QUOTE, decimals: 8 };
    const allocator = strategyAllocator({
      base,
      quote,
      spotPrice: "0.0000125",
      curve: { kind: "fullRange" },
    });

    const fromBase = allocator.fromBase(parseUnits("80000", base.decimals));
    const fromQuote = allocator.fromQuote(parseUnits("1", quote.decimals));

    expect(fromBase).toEqual({
      base: parseUnits("80000", base.decimals),
      quote: parseUnits("1", quote.decimals),
    });
    expect(fromQuote).toEqual(fromBase);
  });

  it("keeps base and quote semantics when the base address sorts last", () => {
    const base = { address: QUOTE, decimals: 8 };
    const quote = { address: BASE, decimals: 6 };
    const allocator = strategyAllocator({
      base,
      quote,
      spotPrice: "80000",
      curve: { kind: "pegged" },
    });

    expect(allocator.fromBase(parseUnits("1", base.decimals))).toEqual({
      base: parseUnits("1", base.decimals),
      quote: parseUnits("80000", quote.decimals),
    });
  });

  it("returns an empty allocation when either available balance is zero", () => {
    const allocator = strategyAllocator({
      base: { address: BASE, decimals: 6 },
      quote: { address: QUOTE, decimals: 6 },
      spotPrice: "1",
      curve: { kind: "fullRange" },
    });

    expect(allocator.fromBase(0n)).toEqual({ base: 0n, quote: 0n });
    expect(allocator.max({ base: parseUnits("1", 6), quote: 0n })).toEqual({
      base: 0n,
      quote: 0n,
    });
  });

  it("rejects negative reserves and never matches them", () => {
    const allocator = strategyAllocator({
      base: { address: BASE, decimals: 6 },
      quote: { address: QUOTE, decimals: 6 },
      spotPrice: "1",
      curve: { kind: "fullRange" },
    });

    expect(() => allocator.fromBase(-1n)).toThrow(RangeError);
    expect(() => allocator.fromQuote(-1n)).toThrow(RangeError);
    expect(() => allocator.max({ base: 1n, quote: -1n })).toThrow(RangeError);
    expect(allocator.matches({ base: -1n, quote: 1n })).toBe(false);
  });

  it("finds the largest paired allocation that fits both balances", () => {
    const base = { address: BASE, decimals: 6 };
    const quote = { address: QUOTE, decimals: 8 };
    const allocator = strategyAllocator({
      base,
      quote,
      spotPrice: "0.0000125",
      curve: { kind: "fullRange" },
    });

    expect(
      allocator.max({
        base: parseUnits("100000", base.decimals),
        quote: parseUnits("0.5", quote.decimals),
      }),
    ).toEqual({
      base: parseUnits("40000", base.decimals),
      quote: parseUnits("0.5", quote.decimals),
    });
  });

  it("recognizes SDK-paired reserves and rejects a one-atom mismatch", () => {
    const allocator = strategyAllocator({
      base: { address: BASE, decimals: 18 },
      quote: { address: QUOTE, decimals: 6 },
      spotPrice: "2500",
      curve: {
        kind: "concentrated",
        priceMin: "1250",
        priceMax: "3750",
      },
    });
    const allocation = allocator.fromBase(parseUnits("1", 18));

    expect(allocator.matches(allocation)).toBe(true);
    expect(
      allocator.matches({
        ...allocation,
        quote: allocation.quote + 1n,
      }),
    ).toBe(false);
  });

  it.each(["fullRange", "concentrated", "pegged"] as const)(
    "%s preserves the requested marginal price and balance limits across token orderings",
    (kind) => {
      const scenarios = [
        {
          base: { address: BASE, decimals: 18 },
          quote: { address: QUOTE, decimals: 6 },
          spotPrice: "2500",
          priceMin: "1250",
          priceMax: "3750",
        },
        {
          base: { address: QUOTE, decimals: 8 },
          quote: { address: BASE, decimals: 6 },
          spotPrice: "80000",
          priceMin: "40000",
          priceMax: "120000",
        },
      ];

      for (const scenario of scenarios) {
        const curve: Parameters<typeof strategyAllocator>[0]["curve"] =
          kind === "concentrated"
            ? {
                kind,
                priceMin: scenario.priceMin,
                priceMax: scenario.priceMax,
              }
            : { kind };
        const allocator = strategyAllocator({ ...scenario, curve });
        const allocation = allocator.fromBase(
          parseUnits("1", scenario.base.decimals),
        );

        expect(allocator.matches(allocation)).toBe(true);
        const expectedSpot = Number(scenario.spotPrice);
        const actualSpot = recoveredSpot(scenario, curve, allocation);
        expect(Math.abs(actualSpot - expectedSpot) / expectedSpot).toBeLessThan(
          1e-8,
        );

        const available = {
          base: parseUnits("2", scenario.base.decimals),
          quote:
            allocator.fromBase(parseUnits("1.5", scenario.base.decimals))
              .quote - 1n,
        };
        const maximum = allocator.max(available);
        expect(maximum.base).toBeLessThanOrEqual(available.base);
        expect(maximum.quote).toBeLessThanOrEqual(available.quote);
        expect(allocator.matches(maximum)).toBe(true);
      }
    },
  );
});

function recoveredSpot(
  scenario: {
    base: { address: Address; decimals: number };
    quote: { address: Address; decimals: number };
    spotPrice: string;
    priceMin: string;
    priceMax: string;
  },
  curve: Parameters<typeof strategyAllocator>[0]["curve"],
  allocation: { base: bigint; quote: bigint },
): number {
  if (curve.kind === "concentrated") {
    const recovered = instructions.concentrate.PriceRange.fromPriceBounds(
      {
        minPrice: instructions.concentrate.Price.fromHuman(
          curve.priceMin,
          pricePair(scenario.base, scenario.quote),
        ),
        maxPrice: instructions.concentrate.Price.fromHuman(
          curve.priceMax,
          pricePair(scenario.base, scenario.quote),
        ),
      },
      {
        reserveA: instructions.concentrate.TokenReserve.new({
          token: new SdkAddress(scenario.base.address),
          reserve: allocation.base,
        }),
        reserveB: instructions.concentrate.TokenReserve.new({
          token: new SdkAddress(scenario.quote.address),
          reserve: allocation.quote,
        }),
      },
    );
    return Number(
      recovered.spotPrice.toHuman(new SdkAddress(scenario.quote.address)),
    );
  }
  return (
    Number(formatUnits(allocation.quote, scenario.quote.decimals)) /
    Number(formatUnits(allocation.base, scenario.base.decimals))
  );
}

function sdkToken(token: { address: Address; decimals: number }) {
  return {
    address: new SdkAddress(token.address),
    decimals: token.decimals,
  };
}

function pricePair(
  base: { address: Address; decimals: number },
  quote: { address: Address; decimals: number },
) {
  return {
    baseToken: { ...sdkToken(base), decimals: BigInt(base.decimals) },
    quoteToken: { ...sdkToken(quote), decimals: BigInt(quote.decimals) },
  };
}
