import { readFileSync } from "node:fs";
import { join } from "node:path";

import { HexString, Order } from "@1inch/swap-vm-sdk";
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
const CREDENTIAL = "0x4444444444444444444444444444444444444444" as Address;
const LO = "0x2222222222222222222222222222222222222222" as Address;
const HI = "0x3333333333333333333333333333333333333333" as Address;
const CREDENTIAL_PREFIX = `0x0e14${CREDENTIAL.slice(2)}`;

function protectedProgram(program: string): string {
  return `${CREDENTIAL_PREFIX}${program.slice(2)}`;
}

function fixtureProgram(encodedOrder: string): string {
  return Order.decode(new HexString(encodedOrder)).program.toString();
}

describe("Strategy.concentrated", () => {
  for (const [i, v] of range.concentrated.entries()) {
    it(`vector ${i} (${v.lowerPrice}..${v.upperPrice}) preserves the SDK curve behind its credential`, () => {
      const built = Strategy.concentrated({
        base: { address: LO, decimals: 18 },
        quote: { address: HI, decimals: 18 },
        priceMin: v.lowerPrice,
        priceMax: v.upperPrice,
      }).build(MAKER, CREDENTIAL);
      expect(built.program).toBe(protectedProgram(fixtureProgram(v.strategyHex)));
    });
  }
});

describe("Strategy.pegged", () => {
  for (const [i, p] of range.pegged.entries()) {
    it(`vector ${i} (dec ${p.decLo}/${p.decHi}, lw ${p.linearWidth}) preserves the SDK curve behind its credential`, () => {
      const built = Strategy.pegged({
        tokenA: { address: LO, decimals: p.decLo, reserve: BigInt(p.reserveLo) },
        tokenB: { address: HI, decimals: p.decHi, reserve: BigInt(p.reserveHi) },
        linearWidth: BigInt(p.linearWidth),
      }).build(MAKER, CREDENTIAL);
      expect(built.program).toBe(protectedProgram(fixtureProgram(p.strategyHex)));
    });
  }
});

describe("Strategy.fullRange + fee", () => {
  const byOpcodes = (ops: number[]) =>
    strategies.strategies.find(
      (s: { curve: string; opcodes: number[] }) =>
        s.curve === "xyc" && JSON.stringify(s.opcodes) === JSON.stringify(ops),
    );

  it("plain full-range preserves the SDK program behind its credential", () => {
    const want = byOpcodes([17]);
    const built = Strategy.fullRange().build(MAKER, CREDENTIAL);
    expect(built.program).toBe(protectedProgram(want.programHex));
  });

  it("full-range with a 30 bps fee preserves the SDK program behind its credential", () => {
    const want = byOpcodes([21, 17]);
    const built = Strategy.fullRange().fee(30).build(MAKER, CREDENTIAL);
    expect(built.program).toBe(protectedProgram(want.programHex));
  });

  it.each([-1, 1.5, 10_000])("rejects an invalid fee of %s bps", (fee) => {
    expect(() => Strategy.fullRange().fee(fee)).toThrow(RangeError);
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
    }).build(MAKER, CREDENTIAL);
    const viaConcentrated = Strategy.concentrated({
      base: { address: LO, decimals: 18 },
      quote: { address: HI, decimals: 18 },
      priceMin: bounds.priceMin,
      priceMax: bounds.priceMax,
    }).build(MAKER, CREDENTIAL);
    expect(viaRange.order).toBe(viaConcentrated.order);
  });
});

describe("protected strategy construction", () => {
  const supported = [
    Strategy.fullRange(),
    Strategy.concentrated({
      base: { address: LO, decimals: 18 },
      quote: { address: HI, decimals: 18 },
      priceMin: "0.95",
      priceMax: "1.05",
    }),
    Strategy.pegged({
      tokenA: { address: LO, decimals: 18, reserve: 100n * 10n ** 18n },
      tokenB: { address: HI, decimals: 18, reserve: 100n * 10n ** 18n },
      linearWidth: linearWidthFromSymmetricRangePercent(1),
    }),
  ];

  it("prefixes every supported curve with the filler-only taker credential", () => {
    for (const strategy of supported) {
      expect(strategy.build(MAKER, CREDENTIAL).program.startsWith(CREDENTIAL_PREFIX)).toBe(true);
    }
  });

  it("rejects an invalid credential before building an order", () => {
    expect(() => Strategy.fullRange().build(MAKER, "0x0" as Address)).toThrow(RangeError);
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
    const original = strategy.build(MAKER, CREDENTIAL);
    const first = strategy.salt(1n).build(MAKER, CREDENTIAL);
    const second = strategy.salt(2n).build(MAKER, CREDENTIAL);

    expect(first.strategyHash).not.toBe(original.strategyHash);
    expect(second.strategyHash).not.toBe(first.strategyHash);
    expect(first.program.startsWith(original.program)).toBe(true);
    expect(second.program.startsWith(original.program)).toBe(true);
    expect(strategy.salt(1n).build(MAKER, CREDENTIAL)).toEqual(first);
    expect(strategy.salt(0n).build(MAKER, CREDENTIAL)).toEqual(original);
    expect(strategy.build(MAKER, CREDENTIAL)).toEqual(original);
  });

  it("composes with fees in either order without mutating either builder", () => {
    const original = strategy.build(MAKER, CREDENTIAL);
    const fee = strategy.fee(30);
    const withFee = fee.build(MAKER, CREDENTIAL);
    const salted = fee.salt(1n).build(MAKER, CREDENTIAL);

    expect(strategy.salt(1n).fee(30).build(MAKER, CREDENTIAL)).toEqual(salted);
    expect(salted.strategyHash).not.toBe(withFee.strategyHash);
    expect(salted.program.startsWith(withFee.program)).toBe(true);
    expect(fee.build(MAKER, CREDENTIAL)).toEqual(withFee);
    expect(strategy.build(MAKER, CREDENTIAL)).toEqual(original);
  });
});

describe("bandToPrices", () => {
  it("brackets the mid symmetrically", () => {
    expect(bandToPrices("100", 5)).toEqual({ priceMin: "95", priceMax: "105" });
  });
  it("handles fractional percents", () => {
    expect(bandToPrices("100", 2.5)).toEqual({ priceMin: "97.5", priceMax: "102.5" });
  });

  it.each([
    ["0", 5],
    ["100", 0],
    ["100", 0.004],
    ["100", 99.999],
    ["100", 100],
    ["100", Number.NaN],
  ])("rejects an invalid mid %s or half-width %s", (mid, halfWidthPct) => {
    expect(() => bandToPrices(mid, halfWidthPct)).toThrow(RangeError);
  });
});

describe("strategy construction boundaries", () => {
  it("rejects duplicate tokens and equal concentrated bounds", () => {
    expect(() =>
      Strategy.concentrated({
        base: { address: LO, decimals: 18 },
        quote: { address: LO, decimals: 18 },
        priceMin: "1",
        priceMax: "2",
      }),
    ).toThrow(RangeError);
    expect(() =>
      Strategy.concentrated({
        base: { address: LO, decimals: 18 },
        quote: { address: HI, decimals: 18 },
        priceMin: "1.0",
        priceMax: "1.00",
      }),
    ).toThrow(RangeError);
  });

  it("rejects reserves or widths outside the pegged program domain", () => {
    const tokenA = { address: LO, decimals: 18, reserve: 1n };
    const tokenB = { address: HI, decimals: 18, reserve: 1n };
    expect(() =>
      Strategy.pegged({ tokenA: { ...tokenA, reserve: 0n }, tokenB, linearWidth: 0n }),
    ).toThrow(RangeError);
    expect(() =>
      Strategy.pegged({
        tokenA,
        tokenB,
        linearWidth: 5_000n * 10n ** 27n + 1n,
      }),
    ).toThrow(RangeError);
  });

  it.each([-1n, 1n << 64n])("rejects salt %s outside uint64", (salt) => {
    expect(() => Strategy.fullRange().salt(salt)).toThrow(RangeError);
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

  it.each([0.01, 50])(
    "supports the create-position custom boundary at %s%%",
    (halfWidthPct) => {
      const linearWidth = linearWidthFromSymmetricRangePercent(halfWidthPct);
      const recovered = symmetricRangePercentFromLinearWidth(linearWidth);

      expect(Math.abs(recovered - halfWidthPct)).toBeLessThan(1e-10);
    },
  );
});
