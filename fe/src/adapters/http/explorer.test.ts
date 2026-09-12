import { beforeEach, describe, expect, it, vi } from "vitest";
import { SolventApiError } from "@solvent/sdk/client";

import assets from "@/data/fixtures/assets.json";

const api = vi.hoisted(() => ({
  solvent: { tradeDetail: vi.fn(), trades: vi.fn() },
  origin: { assets: vi.fn(), config: vi.fn() },
  destination: { assets: vi.fn(), config: vi.fn(), trades: vi.fn() },
  crossChain: { order: vi.fn() },
}));

vi.mock("./client", () => ({
  solventApi: api.solvent,
  crossChainOriginApi: api.origin,
  baseApi: api.destination,
  crossChainApi: api.crossChain,
}));

/** The same fixture, restamped onto another chain. */
const onChain = (chainId: number) => ({
  items: assets.items.map((asset) => ({ ...asset, chain_id: chainId })),
});

const CHAIN_A = { chain_id: 31337, name: "Chain A", logo_uri: null };
const BASE = { chain_id: 31338, name: "Base", logo_uri: null };

beforeEach(() => {
  vi.resetAllMocks();
  api.origin.assets.mockResolvedValue(onChain(31337));
  api.origin.config.mockResolvedValue({ chains: [CHAIN_A] });
  api.destination.assets.mockResolvedValue(onChain(31338));
  api.destination.config.mockResolvedValue({ chains: [BASE] });
});

describe("explorer trade lookup", () => {
  it("reads a chain-qualified strategy's settlements from Base", async () => {
    api.destination.trades.mockResolvedValue({ items: [] });

    const { explorerAdapter } = await import("./explorer");
    await explorerAdapter.trades({
      strategy_hash: "0x01",
      status: "confirmed",
      chainId: 31338,
    });

    expect(api.destination.trades).toHaveBeenCalledWith({
      strategy_hash: "0x01",
      status: "confirmed",
      cursor: undefined,
      limit: 10,
    });
    expect(api.solvent.trades).not.toHaveBeenCalled();
  });

  it("falls back to a SolventX order only when the same-chain trade is missing", async () => {
    const link = assets.items.find((asset) => asset.symbol === "LINK");
    const usdc = assets.items.find((asset) => asset.symbol === "USDC");
    if (!link || !usdc) throw new Error("fixture assets are incomplete");
    api.solvent.tradeDetail.mockRejectedValue(
      new SolventApiError(404, "trade not found"),
    );
    api.crossChain.order.mockResolvedValue({
      order_id: "0x01",
      state: "complete",
      quote: {
        amount_in: "0xde0b6b3a7640000",
        amount_out: "0xaef9e7",
        expires_at_unix: 1_900_000_600,
        origin: { input_token: link.address },
        destination: {
          input_token: link.address,
          output_token: usdc.address,
          amount_in: "0xde0b6b3a7640000",
          sources: [],
        },
      },
    });

    const { explorerAdapter } = await import("./explorer");
    await expect(explorerAdapter.trade("0x01")).resolves.toMatchObject({
      id: "0x01",
      status: "confirmed",
      input: { symbol: "LINK", display: "1", net: "Chain A" },
      output: { symbol: "USDC", display: "11.467239", net: "Base" },
    });
    expect(api.crossChain.order).toHaveBeenCalledWith("0x01");
  });

  it("names a cross-chain asset by its own chain, not by the deployment that served it", async () => {
    const link = assets.items.find((asset) => asset.symbol === "LINK");
    const usdc = assets.items.find((asset) => asset.symbol === "USDC");
    if (!link || !usdc) throw new Error("fixture assets are incomplete");
    // The destination deployment answers with an asset that lives on the origin chain.
    api.destination.assets.mockResolvedValue(onChain(31337));
    api.solvent.tradeDetail.mockRejectedValue(
      new SolventApiError(404, "trade not found"),
    );
    api.crossChain.order.mockResolvedValue({
      order_id: "0x02",
      state: "complete",
      quote: {
        amount_in: "0xde0b6b3a7640000",
        amount_out: "0xaef9e7",
        expires_at_unix: 1_900_000_600,
        origin: { input_token: link.address },
        destination: {
          input_token: link.address,
          output_token: usdc.address,
          amount_in: "0xde0b6b3a7640000",
          sources: [],
        },
      },
    });

    const { explorerAdapter } = await import("./explorer");
    await expect(explorerAdapter.trade("0x02")).resolves.toMatchObject({
      output: { symbol: "USDC", net: "Chain A" },
    });
  });

  it("does not hide a same-chain service failure behind the SolventX API", async () => {
    const failure = new SolventApiError(503, "unavailable");
    api.solvent.tradeDetail.mockRejectedValue(failure);

    const { explorerAdapter } = await import("./explorer");
    await expect(explorerAdapter.trade("0x01")).rejects.toBe(failure);
    expect(api.crossChain.order).not.toHaveBeenCalled();
  });
});
