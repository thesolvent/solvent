import { describe, expect, it } from "vitest";

import fixture from "@/data/fixtures/quote.json";

import { toCrossChainQuote, toQuote } from "./quote";

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

  it("marks an output too small to print rather than showing it as zero", () => {
    expect(toQuote(fixture, 18, input).amountOut).toBe("<0.000001");
  });

  it("reports the output's own USD value, not the input's", () => {
    expect(toQuote(fixture, 6, input).amountOutUsd).toBe(
      fixture.amount_out.usd,
    );
  });
});

const CHAIN_LEG = {
  quote_id: "0x00",
  request_id: "0x00",
  role: "origin",
  local_chain: 1,
  remote_chain: 2,
  input_token: input.tokenIn,
  output_token: input.tokenOut,
  amount_in: "0x1",
  amount_out: "0x1",
  route: "direct",
  block_number: 1,
  expires_at_unix: 0,
  sources: [],
} as const;

const asset = (address: string, price: number | null) =>
  ({ address, decimals: 6, price }) as never;

describe("toCrossChainQuote", () => {
  const aggregate = {
    id: "0x00",
    origin: CHAIN_LEG,
    destination: CHAIN_LEG,
    amount_in: "1000000",
    amount_out: "990000",
    bridge_fee: "0",
    expires_at_unix: 0,
  } as never;

  it("states the router's own impact rather than re-deriving it from USD endpoints", () => {
    // The USD endpoints here imply 1%; the router says 0.25%. The router measured the route, the
    // endpoints only measure the valuation feed's opinion of two tokens.
    const quote = toCrossChainQuote(
      { ...(aggregate as object), price_impact_bps: 25 } as never,
      asset(input.tokenIn, 1),
      asset(input.tokenOut, 1),
      1_000_000n,
    );
    expect(quote.priceImpact).toBe("0.25%");
  });

  it("falls back to the USD endpoints when no leg routed", () => {
    const quote = toCrossChainQuote(
      aggregate,
      asset(input.tokenIn, 1),
      asset(input.tokenOut, 1),
      1_000_000n,
    );
    expect(quote.priceImpact).toBe("1.00%");
  });
});
