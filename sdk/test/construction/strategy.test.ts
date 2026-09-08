import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import {
  Strategy,
  bandToPrices,
  linearWidthFromSymmetricRangePercent,
  symmetricRangePercentFromLinearWidth,
} from "../../src/construction/index";
import type { Address } from "../../src/index";

// The corpora the Rust decoder also validates — so an exact match here closes the round-trip
// SDK-encode → shared fixture → Rust-decode. All entries were generated with these constants.
const FIX = join(import.meta.dirname, "../../../crates/core/tests/fixtures");
const range = JSON.parse(readFileSync(join(FIX, "aqua_range_vectors.json"), "utf8"));
const strategies = JSON.parse(readFileSync(join(FIX, "aqua_strategies.json"), "utf8"));

const MAKER = "0x1111111111111111111111111111111111111111" as Address;
const LO = "0x2222222222222222222222222222222222222222" as Address;
const HI = "0x3333333333333333333333333333333333333333" as Address;

describe("Strategy.concentrated", () => {
  for (const [i, v] of range.concentrated.entries()) {
    it(`vector ${i} (${v.lowerPrice}..${v.upperPrice}) reproduces the SDK order`, () => {
      const built = Strategy.concentrated({
        base: { address: LO, decimals: 18 },
        quote: { address: HI, decimals: 18 },
        priceMin: v.lowerPrice,
        priceMax: v.upperPrice,
      }).build(MAKER);
      expect(built.order).toBe(v.strategyHex);
    });
  }
});

describe("Strategy.pegged", () => {
  for (const [i, p] of range.pegged.entries()) {
    it(`vector ${i} (dec ${p.decLo}/${p.decHi}, lw ${p.linearWidth}) reproduces the SDK order`, () => {
      const built = Strategy.pegged({
        tokenA: { address: LO, decimals: p.decLo, reserve: BigInt(p.reserveLo) },
        tokenB: { address: HI, decimals: p.decHi, reserve: BigInt(p.reserveHi) },
        linearWidth: BigInt(p.linearWidth),
      }).build(MAKER);
      expect(built.order).toBe(p.strategyHex);
    });
  }
});

describe("Strategy.fullRange + fee", () => {
  const byOpcodes = (ops: number[]) =>
    strategies.strategies.find(
      (s: { curve: string; opcodes: number[] }) =>
        s.curve === "xyc" && JSON.stringify(s.opcodes) === JSON.stringify(ops),
    );

  it("plain full-range reproduces program + order", () => {
    const want = byOpcodes([17]);
    const built = Strategy.fullRange().build(MAKER);
    expect(built.program).toBe(want.programHex);
    expect(built.order).toBe(want.strategyHex);
  });

  it("full-range with a 30 bps fee reproduces program + order", () => {
    const want = byOpcodes([21, 17]);
    const built = Strategy.fullRange().fee(30).build(MAKER);
    expect(built.program).toBe(want.programHex);
    expect(built.order).toBe(want.strategyHex);
  });
});

describe("Strategy.inRange", () => {
  it("equals concentrated over the same band-derived bounds", () => {
    const bounds = bandToPrices("3000", 5);
    const viaRange = Strategy.inRange({
      base: { address: LO, decimals: 18 },
      quote: { address: HI, decimals: 18 },
      mid: "3000",
      halfWidthPct: 5,
    }).build(MAKER);
    const viaConcentrated = Strategy.concentrated({
      base: { address: LO, decimals: 18 },
      quote: { address: HI, decimals: 18 },
      priceMin: bounds.priceMin,
      priceMax: bounds.priceMax,
    }).build(MAKER);
    expect(viaRange.order).toBe(viaConcentrated.order);
  });
});

describe.each([
  {
    curve: "concentrated",
    strategy: Strategy.concentrated({
      base: { address: LO, decimals: 18 },
      quote: { address: HI, decimals: 18 },
      priceMin: "0.95",
      priceMax: "1.05",
    }),
  },
  {
    curve: "pegged",
    strategy: Strategy.pegged({
      tokenA: { address: LO, decimals: 18, reserve: 100n * 10n ** 18n },
      tokenB: { address: HI, decimals: 18, reserve: 100n * 10n ** 18n },
      linearWidth: linearWidthFromSymmetricRangePercent(1),
    }),
  },
])("Strategy.salt ($curve)", ({ strategy }) => {
  it("creates repeatable distinct identities while preserving the pricing program", () => {
    const original = strategy.build(MAKER);
    const first = strategy.salt(1n).build(MAKER);
    const second = strategy.salt(2n).build(MAKER);

    expect(first.strategyHash).not.toBe(original.strategyHash);
    expect(second.strategyHash).not.toBe(first.strategyHash);
    expect(first.program.startsWith(original.program)).toBe(true);
    expect(second.program.startsWith(original.program)).toBe(true);
    expect(strategy.salt(1n).build(MAKER)).toEqual(first);
    expect(strategy.salt(0n).build(MAKER)).toEqual(original);
    expect(strategy.build(MAKER)).toEqual(original);
  });

  it("composes with fees in either order without mutating either builder", () => {
    const original = strategy.build(MAKER);
    const fee = strategy.fee(30);
    const withFee = fee.build(MAKER);
    const salted = fee.salt(1n).build(MAKER);

    expect(strategy.salt(1n).fee(30).build(MAKER)).toEqual(salted);
    expect(salted.strategyHash).not.toBe(withFee.strategyHash);
    expect(salted.program.startsWith(withFee.program)).toBe(true);
    expect(fee.build(MAKER)).toEqual(withFee);
    expect(strategy.build(MAKER)).toEqual(original);
  });
});

describe("bandToPrices", () => {
  it("brackets the mid symmetrically", () => {
    expect(bandToPrices("100", 5)).toEqual({ priceMin: "95", priceMax: "105" });
  });
  it("handles fractional percents", () => {
    expect(bandToPrices("100", 2.5)).toEqual({ priceMin: "97.5", priceMax: "102.5" });
  });
});

describe("pegged band % ↔ linearWidth", () => {
  for (const [i, p] of range.pegged.entries()) {
    it(`vector ${i} band % matches and round-trips within tolerance`, () => {
      const lw = BigInt(p.linearWidth);
      expect(symmetricRangePercentFromLinearWidth(lw)).toBe(p.expectedBandPct);
      const back = linearWidthFromSymmetricRangePercent(p.expectedBandPct);
      const diff = back > lw ? back - lw : lw - back;
      // The recovered width matches to better than 1e-10 relative; the SDK rounds the
      // percent to ~13 significant figures, so an absolute bound would not scale with width.
      expect(Number(diff) / Number(lw)).toBeLessThan(1e-10);
    });
  }
});
