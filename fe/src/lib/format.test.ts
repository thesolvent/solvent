import { describe, expect, it } from "vitest";

import {
  blockNumber,
  DASH,
  count,
  percent,
  tokenAmount,
  tokenWithSymbol,
  truncateAddress,
  truncateHash,
  usd,
} from "./format";

const ADDRESS = "0xdc64a140aa3e981100a9beca4e685f962f0cf6c9";
const HASH =
  "0x9f2c1b8a4d6e0f3752c8b1a0d94e7f6531c2a8b0d47e91f6320c5a8b7d1e4f09";

describe("truncateAddress / truncateHash", () => {
  it("shortens an address and a hash to visibly different shapes", () => {
    // The whole point of two truncators: a maker must be able to tell which kind of thing a cell
    // holds. Identical shapes would make this assertion pass trivially.
    expect(truncateAddress(ADDRESS)).toBe("0xdc64…f6c9");
    expect(truncateHash(HASH)).toBe("0x9f2c1b…1e4f09");
    expect(truncateAddress(HASH)).not.toBe(truncateHash(HASH));
  });

  it("renders an absent identifier as the placeholder, not as an empty ellipsis", () => {
    for (const missing of [null, undefined, ""]) {
      expect(truncateAddress(missing)).toBe(DASH);
      expect(truncateHash(missing)).toBe(DASH);
    }
  });

  it("leaves a value shorter than its own truncation alone", () => {
    expect(truncateAddress("0xabcd")).toBe("0xabcd");
    expect(truncateHash("0xabcdef1234")).toBe("0xabcdef1234");
  });
});

describe("usd", () => {
  it("is exact below the compact threshold and compact above it", () => {
    expect(usd(1234.56)).toBe("$1,234.56");
    expect(usd(9999.99)).toBe("$9,999.99");
    expect(usd(12345)).toBe("$12.3K");
    expect(usd(1_250_000)).toBe("$1.3M");
  });

  it("honours an explicit compact choice in both directions", () => {
    expect(usd(12345, { compact: false })).toBe("$12,345.00");
    expect(usd(1234.56, { compact: true })).toBe("$1.2K");
  });

  it("marks an amount too small to show rather than printing it as free", () => {
    expect(usd(0.004)).toBe("<$0.01");
    expect(usd(-0.004)).toBe("-<$0.01");
    // A real zero is a value, and still reads as one.
    expect(usd(0)).toBe("$0.00");
  });

  it("renders an unknown amount as the placeholder, never as zero", () => {
    expect(usd(null)).toBe(DASH);
    expect(usd(undefined)).toBe(DASH);
    expect(usd(Number.NaN)).toBe(DASH);
  });

  it("groups in en-US whatever the host default is", () => {
    // Rebuilding the formatter under a German default must not produce "1.234,56".
    expect(usd(1234.56)).toContain(",");
    expect(usd(1234.56)).toMatch(/^\$\d{1,3}(,\d{3})*\.\d{2}$/);
  });
});

describe("percent", () => {
  it("signs a move the way each surface asks and no other way", () => {
    expect(percent(1.234)).toBe("1.23%");
    expect(percent(1.234, { sign: "plus" })).toBe("+1.23%");
    expect(percent(-1.234, { sign: "plus" })).toBe("-1.23%");
    expect(percent(1.234, { sign: "arrow" })).toBe("↗ 1.23%");
    expect(percent(-1.234, { sign: "arrow" })).toBe("↘ 1.23%");
  });

  it("holds the requested places open so a column stays aligned", () => {
    expect(percent(5, { digits: 1 })).toBe("5.0%");
    expect(percent(5, { digits: 0 })).toBe("5%");
  });

  it("renders an unknown percentage as the placeholder", () => {
    expect(percent(null)).toBe(DASH);
    expect(percent(undefined, { sign: "arrow" })).toBe(DASH);
    expect(percent(Number.POSITIVE_INFINITY)).toBe(DASH);
  });

  it("groups a large percentage in en-US", () => {
    expect(percent(12345.6, { digits: 1 })).toBe("12,345.6%");
  });
});

describe("tokenAmount", () => {
  it("keeps every digit of an amount too long for a double", () => {
    // 21 significant digits: Number() would round this to 123456789012345680000.
    expect(tokenAmount("123456789012345678901.987654")).toBe(
      "123,456,789,012,345,678,901.98",
    );
  });

  it("truncates rather than rounds, so a balance never reads high", () => {
    expect(tokenAmount("1234.999")).toBe("1,234.99");
    expect(tokenAmount("1.99999")).toBe("1.9999");
  });

  it("gives small amounts the places where their information is", () => {
    expect(tokenAmount("0.12345678")).toBe("0.123456");
    expect(tokenAmount("12.5")).toBe("12.5000");
    expect(tokenAmount("5000")).toBe("5,000.00");
  });

  it("marks an amount below the smallest displayable unit instead of printing zero", () => {
    expect(tokenAmount("0.0000001")).toBe("<0.000001");
    expect(tokenAmount("-0.0000001")).toBe("-<0.000001");
    expect(tokenAmount("1e-9")).toBe("<0.000001");
    // A genuine zero is not a rounding artefact and must still read as zero.
    expect(tokenAmount("0")).toBe("0.000000");
  });

  it("renders an absent or unparseable amount as the placeholder", () => {
    expect(tokenAmount(null)).toBe(DASH);
    expect(tokenAmount(undefined)).toBe(DASH);
    expect(tokenAmount("")).toBe(DASH);
    expect(tokenAmount("not a number")).toBe(DASH);
    expect(tokenAmount(Number.NaN)).toBe(DASH);
  });

  it("carries a symbol only when there is an amount to carry it", () => {
    expect(tokenWithSymbol("2.5", "WETH")).toBe("2.5000 WETH");
    expect(tokenWithSymbol(null, "WETH")).toBe(DASH);
  });
});

describe("count", () => {
  it("groups in en-US and marks an unknown count", () => {
    expect(count(1234567)).toBe("1,234,567");
    expect(count(0)).toBe("0");
    expect(count(null)).toBe(DASH);
  });
});

describe("blockNumber", () => {
  it("leaves a block height ungrouped so it reads as an identifier", () => {
    expect(blockNumber(1234567)).toBe("1234567");
    expect(blockNumber(0)).toBe("0");
    expect(blockNumber(null)).toBe(DASH);
  });
});
