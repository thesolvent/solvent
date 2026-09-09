import { describe, expect, it } from "vitest";

import fixture from "@/data/fixtures/quote.json";

import { toQuote } from "./quote";

const input = {
  tokenIn: "0x1111111111111111111111111111111111111111",
  tokenOut: "0x2222222222222222222222222222222222222222",
  amountInRaw: 1_000_000n,
} as const;

describe("toQuote", () => {
  it("scales the raw output by the token's own decimals", () => {
    // 2477852376 at 6 decimals is 2477.85 USDC — reading `display` blind would not catch a
    // decimals mismatch, and scaling by the wrong side is off by a factor of 10^12.
    expect(toQuote(fixture, 6, input).amountOut).toBe("2,477.85");
  });

  it("prints more places when the output is small", () => {
    expect(toQuote(fixture, 18, input).amountOut).toBe("0.000000");
  });

  it("reports the output's own USD value, not the input's", () => {
    expect(toQuote(fixture, 6, input).amountOutUsd).toBe(
      fixture.amount_out.usd,
    );
  });
});
