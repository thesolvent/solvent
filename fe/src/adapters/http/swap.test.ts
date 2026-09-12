import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Asset, Quote } from "@/data";

const api = vi.hoisted(() => ({
  sameChain: { quote: vi.fn() },
  origin: { config: vi.fn(), assets: vi.fn() },
  destination: { config: vi.fn(), assets: vi.fn(), quote: vi.fn() },
  crossChain: { quote: vi.fn(), draft: vi.fn(), submitDirect: vi.fn() },
}));
const sdk = vi.hoisted(() => ({
  createSwapClient: vi.fn(),
  createIntent: vi.fn(),
  submit: vi.fn(),
}));
const compact = vi.hoisted(() => ({
  balance: vi.fn(),
  approve: vi.fn(),
  deposit: vi.fn(),
  sign: vi.fn(),
}));
vi.mock("./client", () => ({
  solventApi: api.sameChain,
  crossChainOriginApi: api.origin,
  baseApi: api.destination,
  crossChainApi: api.crossChain,
}));
vi.mock("@solvent/sdk/swap", () => ({
  createSwapClient: sdk.createSwapClient,
}));
vi.mock("@solvent/sdk/cross-chain", () => ({
  compactBalance: compact.balance,
  approveCompact: compact.approve,
  depositCompact: compact.deposit,
  signCompactMandate: compact.sign,
}));

const from = {
  chainId: 31337,
  address: "0x1111111111111111111111111111111111111111",
  symbol: "USDC",
  name: "USD Coin",
  decimals: 6,
  price: 1,
  change: "0%",
  tags: ["USD"],
  net: "Devnet",
  pairs: ["USDC/WETH"],
} satisfies Asset;
const to = {
  chainId: 31337,
  address: "0x2222222222222222222222222222222222222222",
  symbol: "WETH",
  name: "Wrapped Ether",
  decimals: 18,
  price: 2_500,
  change: "0%",
  tags: ["ETH"],
  net: "Devnet",
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
  sdk.createSwapClient.mockReturnValue({ createIntent: sdk.createIntent });
  sdk.createIntent.mockReturnValue({ submit: sdk.submit });
  sdk.submit.mockResolvedValue({ trade_id: "trade", status: "submitted" });
  compact.balance.mockResolvedValue(0n);
  compact.approve.mockResolvedValue("0xapproval");
  compact.deposit.mockResolvedValue("0xdeposit");
  compact.sign.mockResolvedValue("0xsponsor");
  compact.approve.mockImplementation(
    async (_public, _wallet, _request, options) => {
      options?.onBroadcast();
      return "0xapproval";
    },
  );
  compact.deposit.mockImplementation(
    async (_public, _wallet, _request, options) => {
      options?.onBroadcast();
      return "0xdeposit";
    },
  );
});

describe("swap HTTP adapter invariants", () => {
  it("uses the Base API for a same-chain Base quote and submission", async () => {
    const baseWbtc = {
      ...from,
      chainId: 31338,
      address: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      symbol: "WBTC",
      decimals: 8,
      price: 77_000,
      net: "Base",
    } satisfies Asset;
    const baseUsdc = {
      ...from,
      chainId: 31338,
      address: "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      net: "Base",
    } satisfies Asset;
    api.destination.quote.mockResolvedValue({
      amount_out: { raw: "75631904536", usd: 75_651.19 },
      price_impact_pct: 1.23,
      makers_sourced: 2,
      expires_at: "2100-01-01T00:00:00Z",
    });

    const { swapAdapter } = await import("./swap");
    const priced = await swapAdapter.quote({
      from: baseWbtc,
      to: baseUsdc,
      amount: "1",
    });

    expect(api.sameChain.quote).not.toHaveBeenCalled();
    expect(api.destination.quote).toHaveBeenCalledWith({
      token_in: baseWbtc.address,
      token_out: baseUsdc.address,
      amount_in: "100000000",
    });

    await swapAdapter
      .createIntent(
        {
          from: baseWbtc,
          to: baseUsdc,
          amount: "1",
          quote: priced,
          swapper: "0x3333333333333333333333333333333333333333",
          slippagePct: 0.5,
        },
        {} as never,
      )
      .submit();

    expect(sdk.createSwapClient).toHaveBeenCalledWith(
      expect.objectContaining({ api: api.destination }),
    );
  });

  it("routes a different-chain pair through the direct coordinator quote", async () => {
    const originLink = {
      ...from,
      address: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      symbol: "LINK",
      decimals: 18,
      price: 12,
    } satisfies Asset;
    const baseUsdc = {
      ...from,
      chainId: 31338,
      address: "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      net: "Base",
    } satisfies Asset;
    const baseLink = {
      address: "0xcccccccccccccccccccccccccccccccccccccccc",
      symbol: "LINK",
      decimals: 18,
    };
    api.origin.config.mockResolvedValue({
      chain_id: 31337,
      networks: ["Ethereum"],
    });
    api.destination.config.mockResolvedValue({
      chain_id: 31338,
      networks: ["Base"],
    });
    api.origin.assets.mockResolvedValue({
      items: [{ ...baseLink, address: originLink.address }],
    });
    api.destination.assets.mockResolvedValue({ items: [baseLink] });
    api.crossChain.quote.mockResolvedValue({
      amount_out: "0xb6f5e8",
      expires_at_unix: 1_900_000_000,
      origin: { sources: [] },
      destination: {
        sources: [{ maker: "0xdddddddddddddddddddddddddddddddddddddddd" }],
      },
    });

    const { swapAdapter } = await import("./swap");
    const priced = await swapAdapter.quote({
      from: originLink,
      to: baseUsdc,
      amount: "1",
    });

    expect(api.sameChain.quote).not.toHaveBeenCalled();
    expect(api.crossChain.quote).toHaveBeenCalledWith(
      expect.objectContaining({
        origin_chain_id: 31337,
        destination_chain_id: 31338,
        origin_token_in: originLink.address,
        origin_token_out: originLink.address,
        destination_token_in: baseLink.address,
        destination_token_out: baseUsdc.address,
        amount_in: "0xde0b6b3a7640000",
        destination_amount_in: "0xde0b6b3a7640000",
        route: "direct",
      }),
    );
    expect(priced).toMatchObject({
      tokenIn: originLink.address,
      tokenOut: baseUsdc.address,
      amountInRaw: 1_000_000_000_000_000_000n,
      amountOut: "11.9905",
      amountOutUsd: 11.990504,
      priceImpact: "0.08%",
      amountOutRaw: 11_990_504n,
      makersSourced: 1,
    });
  });

  it("deposits into Compact, signs the mandate, and submits the direct order", async () => {
    const originLink = {
      ...from,
      address: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      symbol: "LINK",
      decimals: 18,
      price: 12,
    } satisfies Asset;
    const baseUsdc = {
      ...from,
      chainId: 31338,
      address: "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      net: "Base",
    } satisfies Asset;
    const aggregate = {
      id: "0x01",
      amount_in: "0xde0b6b3a7640000",
      amount_out: "0xb6f5e8",
      bridge_fee: "0x00",
      expires_at_unix: 1_900_000_000,
      origin: { sources: [] },
      destination: { sources: [] },
    } as never;
    const priced: Quote = {
      ...quote,
      tokenIn: originLink.address,
      tokenOut: baseUsdc.address,
      amountInRaw: 1_000_000_000_000_000_000n,
      amountOutRaw: 11_990_504n,
      expiresAt: 1_900_000_000_000,
      crossChain: aggregate,
    };
    const sponsor = "0x3333333333333333333333333333333333333333";
    api.crossChain.draft.mockResolvedValue({
      compact: "0x4444444444444444444444444444444444444444",
      order: { compact_id: "0x09", origin_chain_id: 31337 },
      commitment: {
        arbiter: "0x5555555555555555555555555555555555555555",
        sponsor,
        nonce: "0x06",
        expires: 1_900_000_900,
        lock_tag: "0x0102030405060708090a0b0c",
        token: originLink.address,
        amount: "0xde0b6b3a7640000",
        mandate: {
          order_id: "0x07",
          destination_chain_id: 31338,
          destination_settler: "0x6666666666666666666666666666666666666666",
          fill_proof_verifier: "0x7777777777777777777777777777777777777777",
          output_token: baseUsdc.address,
          minimum_output_amount: "0xb6f5e8",
          recipient: sponsor,
          fill_deadline: 1_900_000_000,
          exclusive_filler: "0x8888888888888888888888888888888888888888",
          route_kind: 0,
        },
      },
    });
    api.crossChain.submitDirect.mockResolvedValue({
      order_id: "0x07",
      state: "destination_pending",
    });
    const publicClient = {};
    const walletClient = {};
    const statuses: string[] = [];

    const { swapAdapter } = await import("./swap");
    await expect(
      swapAdapter
        .createIntent(
          {
            from: originLink,
            to: baseUsdc,
            amount: "1",
            quote: priced,
            swapper: sponsor,
            slippagePct: 0.5,
          },
          { publicClient, walletClient } as never,
        )
        .submit({ onStatus: (status) => statuses.push(status.kind) }),
    ).resolves.toEqual({
      tradeId: "0x07",
      status: "destination_pending",
    });
    expect(statuses).toEqual([
      "preparing",
      "approving",
      "confirming",
      "submitting",
      "confirming",
      "signing",
      "submitting",
    ]);

    const draftRequest = api.crossChain.draft.mock.calls[0]?.[0];
    expect(draftRequest).toMatchObject({
      quote: aggregate,
      sponsor,
      recipient: sponsor,
      compact_expires_unix: 1_900_000_900,
    });
    expect(compact.approve).toHaveBeenCalledWith(
      publicClient,
      walletClient,
      expect.objectContaining({
        token: originLink.address,
        amount: 1_000_000_000_000_000_000n,
        sponsor,
      }),
      expect.objectContaining({ onBroadcast: expect.any(Function) }),
    );
    expect(compact.deposit).toHaveBeenCalledWith(
      publicClient,
      walletClient,
      expect.objectContaining({
        token: originLink.address,
        amount: 1_000_000_000_000_000_000n,
        sponsor,
      }),
      expect.objectContaining({ onBroadcast: expect.any(Function) }),
    );
    expect(compact.sign).toHaveBeenCalledWith(
      walletClient,
      "0x4444444444444444444444444444444444444444",
      31337,
      expect.objectContaining({ sponsor, amount: 1_000_000_000_000_000_000n }),
    );
    expect(api.crossChain.submitDirect).toHaveBeenCalledWith({
      draft: draftRequest,
      sponsor_signature: "0xsponsor",
    });
  });

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
