import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import {
  buildStrategy,
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

describe("buildStrategy — concentrated", () => {
  for (const [i, v] of range.concentrated.entries()) {
    it(`vector ${i} (${v.lowerPrice}..${v.upperPrice}) reproduces the SDK order`, () => {
      const built = buildStrategy({
        maker: MAKER,
        strategy: {
          curve: "concentrated",
          base: { address: LO, decimals: 18 },
          quote: { address: HI, decimals: 18 },
          priceMin: v.lowerPrice,
          priceMax: v.upperPrice,
        },
      });
      expect(built.order).toBe(v.strategyHex);
    });
  }
});

describe("buildStrategy — pegged", () => {
  for (const [i, p] of range.pegged.entries()) {
    it(`vector ${i} (dec ${p.decLo}/${p.decHi}, lw ${p.linearWidth}) reproduces the SDK order`, () => {
      const built = buildStrategy({
        maker: MAKER,
        strategy: {
          curve: "pegged",
          tokenA: { address: LO, decimals: p.decLo, reserve: BigInt(p.reserveLo) },
          tokenB: { address: HI, decimals: p.decHi, reserve: BigInt(p.reserveHi) },
          linearWidth: BigInt(p.linearWidth),
        },
      });
      expect(built.order).toBe(p.strategyHex);
    });
  }
});

describe("buildStrategy — full-range + fee", () => {
  const byOpcodes = (ops: number[]) =>
    strategies.strategies.find(
      (s: { curve: string; opcodes: number[] }) =>
        s.curve === "xyc" && JSON.stringify(s.opcodes) === JSON.stringify(ops),
    );

  it("plain full-range reproduces program + order", () => {
    const want = byOpcodes([17]);
    const built = buildStrategy({ maker: MAKER, strategy: { curve: "full-range" } });
    expect(built.program).toBe(want.programHex);
    expect(built.order).toBe(want.strategyHex);
  });

  it("full-range with a 30 bps fee reproduces program + order", () => {
    const want = byOpcodes([21, 17]);
    const built = buildStrategy({ maker: MAKER, strategy: { curve: "full-range" }, feeBps: 30 });
    expect(built.program).toBe(want.programHex);
    expect(built.order).toBe(want.strategyHex);
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
