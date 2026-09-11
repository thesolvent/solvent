import { describe, expect, it } from "vitest";

import type { Asset, Quote } from "@/data";

import {
  choices,
  networkOptions,
  settleLegs,
  swapAction,
  tagOptions,
} from "./swap";

function asset(
  symbol: string,
  pairs: string[] = [],
  tags: string[] = [],
  net = "Ethereum",
): Asset {
  return {
    address: `0x${symbol}`,
    symbol,
    name: symbol,
    decimals: 18,
    price: 1,
    change: "+0.00%",
    tags,
    logoUri: null,
    net,
    pairs,
  };
}

const ASSETS = [
  asset("DAI", ["DAI/USDC"], ["Stables"]),
  asset("WETH", ["WETH/USDC"], ["Majors"]),
  asset("USDT", [], ["Stables"]),
  asset("USDC", ["DAI/USDC", "WETH/USDC"], ["Stables"]),
];

describe("filter vocabularies", () => {
  // A fixed list offers tags no asset carries and chains the deployment does not serve.
  it("offers only what the served assets carry", () => {
    expect(tagOptions(ASSETS)).toEqual(["All", "Majors", "Stables"]);
    expect(networkOptions(ASSETS)).toEqual(["All networks", "Ethereum"]);
  });

  it("collapses to the unfiltered choice when nothing is served", () => {
    expect(tagOptions([])).toEqual(["All"]);
  });
});

describe("pair constraints", () => {
  it("keeps an asset that leads nowhere out of the input leg", () => {
    const offered = choices(ASSETS, "from", "DAI").map((a) => a.symbol);
    expect(offered).not.toContain("USDT");
  });

  it("offers the output leg only what the input is quotable against", () => {
    // Which also means it can never offer the asset already on the other leg.
    expect(choices(ASSETS, "to", "DAI").map((a) => a.symbol)).toEqual(["USDC"]);
    expect(choices(ASSETS, "to", "USDC").map((a) => a.symbol)).toEqual([
      "DAI",
      "WETH",
    ]);
  });
});

describe("settling the legs", () => {
  it("leaves a pair the deployment quotes alone", () => {
    expect(settleLegs(ASSETS, "DAI", "USDC")).toBeNull();
  });

  it("clears the output leg when the input one no longer pairs with it", () => {
    expect(settleLegs(ASSETS, "WETH", "DAI")).toEqual({
      fromToken: "WETH",
      toToken: "",
    });
  });

  it("selects only a valid source when neither leg is served", () => {
    expect(settleLegs(ASSETS, "", "")).toEqual({
      fromToken: "DAI",
      toToken: "",
    });
  });

  it("waits for the assets rather than guessing", () => {
    expect(settleLegs([], "ETH", "SOL")).toBeNull();
  });
});

describe("the action button", () => {
  const QUOTE = {
    tokenIn: "0x1111111111111111111111111111111111111111",
    tokenOut: "0x2222222222222222222222222222222222222222",
    amountInRaw: 1_000_000n,
    amountOut: "1",
    amountOutUsd: 1,
    priceImpact: "0.1%",
    makersSourced: 1,
    amountOutRaw: 1_000_000n,
    expiresAt: 0,
  } satisfies Quote;

  const base = {
    connected: true,
    switchTo: undefined,
    submitting: false,
    submitted: false,
    amount: 1,
    pricing: false,
    quote: QUOTE,
    problem: undefined,
  };

  it("carries the server's reason rather than failing silently", () => {
    const blocked = swapAction({
      ...base,
      quote: undefined,
      problem: "No route for this pair and size",
    });
    expect(blocked).toEqual({
      label: "No route for this pair and size",
      ready: false,
    });
  });

  it("keeps saying why while a failed price is retried", () => {
    const retrying = swapAction({
      ...base,
      pricing: true,
      problem: "No route",
    });
    expect(retrying.label).toBe("No route");
  });

  // Why a trade cannot happen is about the trade, not about who is holding the tokens.
  it("says why a trade is impossible even with no wallet attached", () => {
    const blocked = swapAction({
      ...base,
      connected: false,
      quote: undefined,
      problem: "No route",
    });
    expect(blocked.label).toBe("No route");
  });

  it("asks for the right network before it asks for a signature", () => {
    // A wallet refuses to sign a domain naming a chain it is not on, so never get that far.
    expect(swapAction({ ...base, switchTo: "Solvent Devnet" })).toEqual({
      label: "Switch to Solvent Devnet",
      ready: true,
    });
  });
});
