import { beforeEach, describe, expect, it, vi } from "vitest";

import type { CreatePair, PositionInput } from "@/ports/positions";

const sdk = vi.hoisted(() => ({
  createIntent: vi.fn(),
  submit: vi.fn(),
}));
const api = vi.hoisted(() => ({
  config: vi.fn(),
  pairs: vi.fn(),
  pairHistory: vi.fn(),
  assets: vi.fn(),
  pools: vi.fn(),
}));
const construction = vi.hoisted(() => {
  const builder = {
    fee: vi.fn(),
    salt: vi.fn(),
    build: vi.fn(),
  };
  return {
    builder,
    matches: vi.fn(),
    strategyAllocator: vi.fn(),
    concentrated: vi.fn(),
    fullRange: vi.fn(),
    pegged: vi.fn(),
    linearWidth: vi.fn(),
  };
});
vi.mock("@solvent/sdk/positions", () => ({
  createPositionClient: () => ({ createIntent: sdk.createIntent }),
}));
vi.mock("@solvent/sdk/construction", () => ({
  Strategy: {
    concentrated: construction.concentrated,
    fullRange: construction.fullRange,
    pegged: construction.pegged,
  },
  strategyAllocator: construction.strategyAllocator,
  linearWidthFromSymmetricRangePercent: construction.linearWidth,
}));
vi.mock("./client", () => ({ solventApi: api }));

const MAKER = "0x1111111111111111111111111111111111111111";
const CREDENTIAL = "0x6666666666666666666666666666666666666666";
const MAX_UINT64 = (1n << 64n) - 1n;
const pair: CreatePair = {
  base: {
    address: "0x2222222222222222222222222222222222222222",
    decimals: 18,
    symbol: "WETH",
    name: "Wrapped Ether",
    tags: ["ETH"],
    balance: 2,
    balanceRaw: 2_000_000_000_000_000_000n,
    valueUsd: 5_000,
    changePct: 1,
    tint: "#eee",
  },
  quote: {
    address: "0x3333333333333333333333333333333333333333",
    decimals: 6,
    symbol: "USDC",
    name: "USD Coin",
    tags: ["USD"],
    balance: 10_000,
    balanceRaw: 10_000_000_000n,
    valueUsd: 10_000,
    changePct: 0,
    tint: "#eef",
  },
  mid: 2_500,
  tvlUsd: 500_000,
  type: "Volatile",
  defaultBandPct: 5,
  defaultFeeBps: 5,
};
const input: PositionInput = {
  maker: MAKER,
  pair,
  curve: "Concentrated",
  feeBps: 5,
  spotPrice: "2500",
  priceMin: "2400",
  priceMax: "2600",
  halfWidthPct: 5,
  peggedSymmetric: true,
  amountBase: "1.25",
  amountQuote: "3125.5",
};

beforeEach(() => {
  vi.resetAllMocks();
  api.config.mockResolvedValue({ taker_credential: CREDENTIAL });
  api.assets.mockResolvedValue({ items: [] });
  api.pairs.mockResolvedValue({ items: [] });
  api.pairHistory.mockResolvedValue({ points: [] });
  api.pools.mockResolvedValue({ items: [] });
  construction.concentrated.mockReturnValue(construction.builder);
  construction.fullRange.mockReturnValue(construction.builder);
  construction.pegged.mockReturnValue(construction.builder);
  construction.linearWidth.mockReturnValue(123n);
  construction.builder.fee.mockReturnValue(construction.builder);
  construction.builder.salt.mockReturnValue(construction.builder);
  construction.builder.build.mockReturnValue({
    program: "0x01",
    strategyHash: "0x02",
    order: "0x03",
    maker: MAKER,
  });
  construction.matches.mockReturnValue(true);
  construction.strategyAllocator.mockReturnValue({
    matches: construction.matches,
  });
  sdk.createIntent.mockReturnValue({ submit: sdk.submit });
  sdk.submit.mockResolvedValue({
    strategyHash: "0xstrategy",
    transactionHash: "0xtx",
  });
  vi.spyOn(globalThis.crypto, "getRandomValues").mockImplementation((array) => {
    (array as Uint32Array).fill(1);
    return array;
  });
});

describe("positionsAdapter", () => {
  it("maps timestamped pair history without changing its price orientation", async () => {
    api.pairHistory.mockResolvedValue({
      points: [
        { timestamp_ms: 123, price: 0.0000125, volume_usd: 42 },
        { timestamp_ms: 456, price: 0.0000126, volume_usd: null },
      ],
    });
    const { positionsAdapter } = await import("./positions");

    await expect(positionsAdapter.history(pair, "7d")).resolves.toEqual([
      { timestampMs: 123, price: 0.0000125, volumeUsd: 42 },
      { timestampMs: 456, price: 0.0000126, volumeUsd: undefined },
    ]);
    expect(api.pairHistory).toHaveBeenCalledWith({
      base: pair.base.address,
      quote: pair.quote.address,
      period: "7d",
    });
  });

  it("joins live pool TVL to supported pairs by token address", async () => {
    api.pairs.mockResolvedValue({
      items: [
        {
          base: pair.base,
          quote: pair.quote,
          mid: pair.mid,
          type: "volatile",
          default_band_pct: pair.defaultBandPct,
          default_fee_bps: pair.defaultFeeBps,
          wallet: {
            base: pair.base.balance,
            quote: pair.quote.balance,
            base_raw: pair.base.balanceRaw.toString(),
            quote_raw: pair.quote.balanceRaw.toString(),
          },
        },
      ],
    });
    api.pools.mockResolvedValue({
      items: [
        {
          base: pair.quote,
          quote: pair.base,
          tvl_usd: 750_000,
        },
      ],
    });
    const { positionsAdapter } = await import("./positions");

    await expect(positionsAdapter.pairs(MAKER)).resolves.toMatchObject([
      {
        tvlUsd: 750_000,
        base: { balanceRaw: pair.base.balanceRaw },
        quote: { balanceRaw: pair.quote.balanceRaw },
      },
    ]);
    expect(api.pairs).toHaveBeenCalledWith({ wallet: MAKER });
  });

  it("encodes the selected curve and token decimals into one stable SDK intent", async () => {
    const { positionsAdapter } = await import("./positions");
    const intent = positionsAdapter.createIntent(input, {} as never);

    await intent.submit();
    await intent.submit();

    const salt = new Uint32Array(2)
      .fill(1)
      .reduce((value, word) => (value << 32n) | BigInt(word), 0n);
    expect(construction.concentrated).toHaveBeenCalledWith({
      base: { address: pair.base.address, decimals: 18 },
      quote: { address: pair.quote.address, decimals: 6 },
      priceMin: "2400",
      priceMax: "2600",
    });
    expect(construction.strategyAllocator).toHaveBeenCalledWith({
      base: { address: pair.base.address, decimals: 18 },
      quote: { address: pair.quote.address, decimals: 6 },
      spotPrice: "2500",
      curve: {
        kind: "concentrated",
        priceMin: "2400",
        priceMax: "2600",
      },
    });
    expect(construction.builder.fee).toHaveBeenCalledWith(5);
    expect(construction.builder.salt).toHaveBeenCalledWith(salt);
    expect(construction.builder.build).toHaveBeenCalledWith(MAKER, CREDENTIAL);
    expect(sdk.createIntent).toHaveBeenCalledOnce();
    expect(sdk.createIntent).toHaveBeenCalledWith({
      maker: MAKER,
      strategy: {
        program: "0x01",
        strategyHash: "0x02",
        order: "0x03",
        maker: MAKER,
      },
      amounts: [
        { token: pair.base.address, amount: 1_250_000_000_000_000_000n },
        { token: pair.quote.address, amount: 3_125_500_000n },
      ],
    });
    expect(sdk.submit).toHaveBeenCalledTimes(2);
  });

  it("uses the same full-range allocation curve that it encodes", async () => {
    const { positionsAdapter } = await import("./positions");
    const intent = positionsAdapter.createIntent(
      {
        ...input,
        flipped: true,
        curve: "Full range",
        spotPrice: "0.0004",
        amountBase: "2500",
        amountQuote: "1",
      },
      {} as never,
    );

    await intent.submit();

    expect(construction.strategyAllocator).toHaveBeenCalledWith({
      base: { address: pair.quote.address, decimals: 6 },
      quote: { address: pair.base.address, decimals: 18 },
      spotPrice: "0.0004",
      curve: { kind: "fullRange" },
    });
    expect(construction.fullRange).toHaveBeenCalledOnce();
    expect(sdk.createIntent).toHaveBeenCalledWith(
      expect.objectContaining({
        amounts: [
          { token: pair.quote.address, amount: 2_500_000_000n },
          {
            token: pair.base.address,
            amount: 1_000_000_000_000_000_000n,
          },
        ],
      }),
    );
  });

  it("encodes pegged reserves and width from the allocation it validates", async () => {
    const { positionsAdapter } = await import("./positions");
    const intent = positionsAdapter.createIntent(
      {
        ...input,
        curve: "Pegged",
        halfWidthPct: 50,
        peggedSymmetric: true,
      },
      {} as never,
    );

    await intent.submit();

    expect(construction.strategyAllocator).toHaveBeenCalledWith({
      base: { address: pair.base.address, decimals: 18 },
      quote: { address: pair.quote.address, decimals: 6 },
      spotPrice: "2500",
      curve: { kind: "pegged" },
    });
    expect(construction.linearWidth).toHaveBeenCalledWith(50);
    expect(construction.pegged).toHaveBeenCalledWith({
      tokenA: {
        address: pair.base.address,
        decimals: 18,
        reserve: 1_250_000_000_000_000_000n,
      },
      tokenB: {
        address: pair.quote.address,
        decimals: 6,
        reserve: 3_125_500_000n,
      },
      linearWidth: 123n,
    });
  });

  it("generates a nonzero uint64 strategy salt accepted by SwapVM", async () => {
    construction.builder.salt.mockImplementationOnce((salt: bigint) => {
      if (salt < 1n || salt > MAX_UINT64) {
        throw new Error(`Invalid salt value: ${salt}. Must be a valid uint64`);
      }
      return construction.builder;
    });
    const { positionsAdapter } = await import("./positions");

    const intent = positionsAdapter.createIntent(input, {} as never);

    await expect(intent.submit()).resolves.toMatchObject({
      strategyHash: "0xstrategy",
    });
  });

  it("rejects asymmetric pegged bounds before creating an SDK intent", async () => {
    const { positionsAdapter } = await import("./positions");
    const intent = positionsAdapter.createIntent(
      { ...input, curve: "Pegged", peggedSymmetric: false },
      {} as never,
    );

    await expect(intent.submit()).rejects.toThrow(
      "Pegged positions require a symmetric range",
    );
    expect(sdk.createIntent).not.toHaveBeenCalled();
  });

  it("rejects reserves that do not match the selected curve and spot price", async () => {
    construction.matches.mockReturnValue(false);
    const { positionsAdapter } = await import("./positions");
    const intent = positionsAdapter.createIntent(input, {} as never);

    await expect(intent.submit()).rejects.toThrow(
      "Deposit amounts do not match the selected curve at the live market price",
    );
    expect(sdk.createIntent).not.toHaveBeenCalled();
  });
});
