import { describe, expect, it } from "vitest";

import type { Asset, Quote } from "@/data";

import {
  assetKey,
  impactLevel,
  isAboveBalance,
  isAmountDraft,
  minimumOutput,
  minimumReceived,
  choices,
  networkOptions,
  settleLegs,
  submissionProblem,
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
    chainId: net === "Base" ? 31338 : 31337,
    address: `0x${symbol}`,
    symbol,
    name: symbol,
    decimals: 18,
    price: 1,
    change: "+0.00%",
    tags,
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

  it("keeps direct swaps on one network and unlocks remote outputs for SolventX", () => {
    const multiNetwork = [
      asset("WETH", [], [], "Ethereum"),
      asset("WETH", ["WETH/USDC"], [], "Base"),
      asset("USDC", ["WETH/USDC"], [], "Base"),
    ];

    expect(choices(multiNetwork, "to", "WETH")).toEqual([]);
    expect(choices(multiNetwork, "to", "WETH", true)).toEqual([
      multiNetwork[2],
    ]);
    expect(choices(multiNetwork, "from", "WETH", true)).toEqual([
      multiNetwork[0],
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

  it("clears a remote destination when returning to Solvent", () => {
    const multiNetwork = [
      asset("WETH", [], [], "Ethereum"),
      asset("WETH", ["WETH/USDC"], [], "Base"),
      asset("USDC", ["WETH/USDC"], [], "Base"),
    ];

    expect(settleLegs(multiNetwork, "WETH", "USDC", true)).toBeNull();
    expect(settleLegs(multiNetwork, "WETH", "USDC")).toEqual({
      fromToken: "WETH",
      toToken: "",
    });
  });

  it("keeps equal symbols on different chains as distinct legs", () => {
    const ethereumWeth = asset("WETH", [], [], "Ethereum");
    const baseWeth = asset("WETH", ["WETH/USDC"], [], "Base");
    const baseUsdc = asset("USDC", ["WETH/USDC"], [], "Base");
    const assets = [ethereumWeth, baseWeth, baseUsdc];

    expect(
      settleLegs(assets, assetKey(ethereumWeth), assetKey(baseUsdc), true),
    ).toBeNull();
  });
});

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

describe("the action button", () => {
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

describe("submission errors", () => {
  it("shows a cross-chain coordinator rejection instead of hiding it", () => {
    const error = new Error("Quote expires too soon; request a fresh price");
    error.name = "CrossChainApiError";

    expect(submissionProblem(error)).toBe(
      "Quote expires too soon; request a fresh price",
    );
  });
});

describe("minimum received", () => {
  it("prints the floor the order will enforce, not the quoted output", () => {
    // 0.5% off a 2,477.852376 USDC quote, floored in base units.
    expect(minimumOutput(2_477_852_376n, 0.5)).toBe(2_465_463_114n);
    expect(
      minimumReceived({ ...QUOTE, amountOutRaw: 2_477_852_376n }, 6, 0.5),
    ).toBe("2,465.46");
  });

  it("never promises a floor above the quote when slippage is unusable", () => {
    expect(minimumOutput(1_000n, Number.NaN)).toBe(0n);
    expect(minimumOutput(1_000n, -5)).toBe(1_000n);
  });
});

describe("insufficient funds", () => {
  it("reads the amount at the token's own precision", () => {
    // 1 USDC against a 0.5 USDC balance, both in 6-decimal base units.
    expect(isAboveBalance("1", 6, 500_000n)).toBe(true);
    expect(isAboveBalance("0.4", 6, 500_000n)).toBe(false);
  });

  it("blocks nothing while the holding is unknown", () => {
    expect(isAboveBalance("1000000", 18, undefined)).toBe(false);
  });

  it("says which token is short instead of letting the wallet fail", () => {
    expect(
      swapAction({
        connected: true,
        switchTo: undefined,
        submitting: false,
        submitted: false,
        amount: 1,
        pricing: false,
        quote: QUOTE,
        problem: undefined,
        short: "WETH",
      }),
    ).toEqual({ label: "Insufficient WETH balance", ready: false });
  });
});

describe("price impact", () => {
  it("bands an impact by what it should do to the decision", () => {
    expect(impactLevel("0.12%")).toBe("normal");
    expect(impactLevel("1.00%")).toBe("high");
    expect(impactLevel("24.10%")).toBe("severe");
    expect(impactLevel(undefined)).toBe("normal");
  });

  it("asks for a second press before signing a severe impact", () => {
    const trade = {
      connected: true,
      switchTo: undefined,
      submitting: false,
      submitted: false,
      amount: 1,
      pricing: false,
      quote: QUOTE,
      problem: undefined,
      impact: "severe" as const,
    };

    expect(swapAction(trade)).toEqual({
      label: "Confirm price impact",
      ready: true,
    });
    expect(swapAction({ ...trade, impactAcknowledged: true }).label).toBe(
      "Swap",
    );
  });
});

describe("isAmountDraft", () => {
  it("accepts a decimal still being typed", () => {
    expect(isAmountDraft("")).toBe(true);
    expect(isAmountDraft("0.")).toBe(true);
    expect(isAmountDraft(".")).toBe(true);
    expect(isAmountDraft("100.25")).toBe(true);
  });

  it("refuses what inputMode only discourages", () => {
    expect(isAmountDraft("abc")).toBe(false);
    expect(isAmountDraft("1e9")).toBe(false);
    expect(isAmountDraft("1.2.3")).toBe(false);
    expect(isAmountDraft("-1")).toBe(false);
  });
});
