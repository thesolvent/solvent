import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Asset, Quote } from "@/data";

const api = vi.hoisted(() => ({ quote: vi.fn() }));
const sdk = vi.hoisted(() => ({ createIntent: vi.fn(), submit: vi.fn() }));
vi.mock("./client", () => ({ solventApi: api }));
vi.mock("@solvent/sdk/swap", () => ({
  createSwapClient: () => ({ createIntent: sdk.createIntent }),
}));

const from = {
  address: "0x1111111111111111111111111111111111111111",
  symbol: "USDC",
  name: "USD Coin",
  decimals: 6,
  price: 1,
  change: "0%",
  tags: ["USD"],
  net: "Devnet",
  logoUri: null,
  pairs: ["USDC/WETH"],
} satisfies Asset;
const to = {
  address: "0x2222222222222222222222222222222222222222",
  symbol: "WETH",
  name: "Wrapped Ether",
  decimals: 18,
  price: 2_500,
  change: "0%",
  tags: ["ETH"],
  net: "Devnet",
  logoUri: null,
  pairs: ["USDC/WETH"],
} satisfies Asset;
const quote: Quote = {
  tokenIn: from.address,
  tokenOut: to.address,
  amountInRaw: 1_250_000n,
  amountOut: "2.5",
  amountOutUsd: 2.5,
  amountOutRaw: 2_500_000_000_000_000_000n,
  priceImpact: "0.01%",
  makersSourced: 1,
  expiresAt: Date.now() + 60_000,
};

beforeEach(() => {
  vi.resetAllMocks();
  sdk.createIntent.mockReturnValue({ submit: sdk.submit });
  sdk.submit.mockResolvedValue({ trade_id: "trade", status: "submitted" });
});

describe("swap HTTP adapter invariants", () => {
  it("binds a quote to its exact pair and raw input", async () => {
    const { swapAdapter } = await import("./swap");
    await swapAdapter
      .createIntent(
        {
          from,
          to,
          amount: "1.25",
          quote,
          swapper: "0x3333333333333333333333333333333333333333",
          slippagePct: 0.5,
        },
        {} as never,
      )
      .submit();

    expect(sdk.createIntent).toHaveBeenCalledWith(
      expect.objectContaining({
        amountIn: 1_250_000n,
        minAmountOut: 2_487_500_000_000_000_000n,
      }),
    );
  });

  it.each([
    {
      name: "excess token precision",
      amount: "1.0000001",
      quote,
      slippagePct: 1,
      message: "Swap amount supports at most 6 decimal places",
    },
    {
      name: "quote/input mismatch",
      amount: "1.5",
      quote,
      slippagePct: 1,
      message: "The quote does not match the current swap inputs",
    },
    {
      name: "expired quote",
      amount: "1.25",
      quote: { ...quote, expiresAt: 1 },
      slippagePct: 1,
      message: "The quote expired; request a fresh price",
    },
    {
      name: "rounded 100% slippage",
      amount: "1.25",
      quote,
      slippagePct: 99.999,
      message: "Slippage must resolve to 0 through 9,999 basis points",
    },
    {
      name: "zero minimum output",
      amount: "1.25",
      quote: { ...quote, amountOutRaw: 1n },
      slippagePct: 50,
      message: "minimum output must be greater than zero",
    },
  ])("rejects $name before SDK authorization", async (bad) => {
    const { swapAdapter } = await import("./swap");
    const submission = swapAdapter
      .createIntent(
        {
          from,
          to,
          amount: bad.amount,
          quote: bad.quote,
          swapper: "0x3333333333333333333333333333333333333333",
          slippagePct: bad.slippagePct,
        },
        {} as never,
      )
      .submit();
    await expect(submission).rejects.toMatchObject({
      name: "InputValidationError",
      message: bad.message,
    });
    expect(sdk.createIntent).not.toHaveBeenCalled();
  });
});
