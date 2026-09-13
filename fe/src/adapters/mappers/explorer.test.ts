import { describe, expect, it } from "vitest";

import detail from "@/data/fixtures/trade-detail.json";
import trades from "@/data/fixtures/trades.json";
import activity from "@/data/fixtures/activity.json";
import assets from "@/data/fixtures/assets.json";
import type { Asset } from "@/data";
import { toAsset } from "./asset";
import type { CrossChainOrder } from "@solvent/sdk/cross-chain";
import { toActivity, toCrossChainTrade, toTrade } from "./explorer";

describe("Explorer records", () => {
  it("keeps settlement amounts, profit denomination, and distinct maker shares", () => {
    const trade = toTrade({
      ...detail,
      price_impact_pct: 0,
      legs: [detail.legs[0], detail.legs[0]],
    });
    expect(trade.input).toEqual({ symbol: "DAI", display: "5000" });
    expect(trade.output).toEqual({ symbol: "USDC", display: "4946.047017" });
    expect(trade.surplus).toEqual({
      symbol: "DAI",
      display: "49.634363660778046388",
    });
    expect(trade.makers).toBe(1);
    expect(trade.legs?.map((leg) => leg.sharePct)).toEqual([50, 50]);
    expect(trade.priceImpactPct).toBe(0);
    expect(trade.deadlineAt).toBe(detail.deadline_block);
    expect(trade.orderHash).toBe(detail.order_hash);
  });

  it("keeps missing detail and metrics unknown instead of manufacturing zeroes", () => {
    const trade = toTrade({
      ...trades.items[0],
      status: "submitted",
      surplus: undefined,
      price_impact_pct: undefined,
    });
    expect(trade.makers).toBeNull();
    expect(trade.priceImpactPct).toBeNull();
    expect(trade.surplus).toBeNull();
    expect(trade.lifecycle).toEqual([]);
    expect(trade.legs).toEqual([]);
  });

  it("distinguishes multiple token movements in one transaction", () => {
    const rows = activity.items.map(toActivity);
    expect(rows[0].id).not.toBe(rows[1].id);
    expect(rows[0].txHash).toBe(rows[1].txHash);
    expect(rows[0].amount).toEqual({
      symbol: "DAI",
      display: "4949.995002086627513862",
    });
    expect(rows[1].amount).toEqual({ symbol: "USDC", display: "4946.047017" });
    expect(rows[0].maker).toBe(activity.items[0].maker);
  });

  it("maps output-denominated SolventX reservations into the existing trade slots", () => {
    const link = assets.items.find((asset) => asset.symbol === "LINK");
    const usdc = assets.items.find((asset) => asset.symbol === "USDC");
    if (!link || !usdc) throw new Error("fixture assets are incomplete");
    const linkAddress = link.address as `0x${string}`;
    const usdcAddress = usdc.address as `0x${string}`;
    const order = {
      order_id:
        "0xf969850128c5f806c513dc6db68ced83cf92fdf84dd6f23d73c1d920bb33e0fd",
      taker: "0x3333333333333333333333333333333333333333",
      state: "complete",
      quote: {
        id: "0x01",
        amount_in: "0xde0b6b3a7640000",
        amount_out: "0xaef9e7",
        bridge_fee: "0x00",
        expires_at_unix: 1_900_000_600,
        origin: {
          quote_id: "0x02",
          request_id: "0x03",
          role: "origin",
          local_chain: 31337,
          remote_chain: 31338,
          input_token: linkAddress,
          output_token: linkAddress,
          amount_in: "0xde0b6b3a7640000",
          amount_out: "0xde0b6b3a7640000",
          route: "direct",
          block_number: 10,
          expires_at_unix: 1_900_000_600,
          sources: [],
        },
        destination: {
          quote_id: "0x04",
          request_id: "0x03",
          role: "destination",
          local_chain: 31338,
          remote_chain: 31337,
          input_token: linkAddress,
          output_token: usdcAddress,
          amount_in: "0xde0b6b3a7640000",
          amount_out: "0xaef9e7",
          route: "direct",
          block_number: 11,
          expires_at_unix: 1_900_000_600,
          sources: [
            {
              maker: "0x3c44cdddb6a900fa2b585dd299e03d12fa4293bc",
              strategy_hash:
                "0xdd59bd7696ff1b09859abac95c3ae58cb8b63a6b393af7c032aa03b4eefef0fd",
              token: usdcAddress,
              amount: "0xaef9e7",
            },
          ],
        },
      },
      origin: {
        command_id: "0x05",
        transaction_hash:
          "0x243a599bce767f1783a3e5c66348496e349c8a1539815209a498673d3b4f471c",
        block_number: 148,
      },
      destination: { command_id: "0x06", block_number: 74 },
      fill_proof: { command_id: "0x07", block_number: 163 },
      repayment: { command_id: "0x08", block_number: 217 },
      lifecycle: [
        { stage: "quoted", at: 1_900_000_000 },
        { stage: "destination_fill", at: 1_900_000_030 },
        { stage: "proof_relay", at: 1_900_000_060 },
        { stage: "origin_claim", at: 1_900_000_090 },
        { stage: "repayment", at: 1_900_000_120 },
        { stage: "complete", at: 1_900_000_150 },
      ],
    } satisfies CrossChainOrder;

    const trade = toCrossChainTrade(
      order,
      assets.items.map((asset) => toAsset(asset, "Chain A")) as Asset[],
      assets.items.map((asset) => toAsset(asset, "Base")) as Asset[],
    );

    expect(trade).toMatchObject({
      status: "confirmed",
      taker: "0x3333333333333333333333333333333333333333",
      input: { symbol: "LINK", display: "1" },
      output: { symbol: "USDC", display: "11.467239" },
      surplus: { symbol: "USD" },
      makers: 1,
      txHash:
        "0x243a599bce767f1783a3e5c66348496e349c8a1539815209a498673d3b4f471c",
      blockNumber: 148,
      createdAt: 1_900_000_000,
      settledAt: 1_900_000_150,
    });
    expect(trade.lifecycle).toHaveLength(6);
    expect(trade.lifecycle.map((stage) => stage.status)).toEqual([
      "quoted",
      "destination fill",
      "proof relay",
      "origin claim",
      "repayment",
      "complete",
    ]);
    expect(trade.lifecycle.map((stage) => stage.at)).toEqual([
      1_900_000_000, 1_900_000_030, 1_900_000_060, 1_900_000_090, 1_900_000_120,
      1_900_000_150,
    ]);
    expect(Number(trade.surplus?.display)).toBeCloseTo(1.298942267515, 10);
    expect(trade.legs).toEqual([
      expect.objectContaining({
        input: expect.objectContaining({
          symbol: "LINK",
          display: "1",
          net: "Base",
        }),
        output: expect.objectContaining({
          symbol: "USDC",
          display: "11.467239",
          net: "Base",
        }),
        chainId: 31338,
        strategySource: "direct",
        sharePct: 100,
      }),
    ]);
  });

  it("does not record SolventX lifecycle stages whose evidence is null", () => {
    const link = assets.items.find((asset) => asset.symbol === "LINK");
    const usdc = assets.items.find((asset) => asset.symbol === "USDC");
    if (!link || !usdc) throw new Error("fixture assets are incomplete");
    const linkAddress = link.address as `0x${string}`;
    const usdcAddress = usdc.address as `0x${string}`;
    const order = {
      order_id:
        "0xe1e00281e2666b17064dbb05d48296871285855db30cc3f8a2f10a18e25f7df5",
      state: "fill_proof_pending",
      quote: {
        id: "0x01",
        amount_in: "0xde0b6b3a7640000",
        amount_out: "0xaef7a7",
        bridge_fee: "0x00",
        expires_at_unix: 1_900_000_600,
        origin: {
          quote_id: "0x02",
          request_id: "0x03",
          role: "origin",
          local_chain: 31337,
          remote_chain: 31338,
          input_token: linkAddress,
          output_token: linkAddress,
          amount_in: "0xde0b6b3a7640000",
          amount_out: "0xde0b6b3a7640000",
          route: "direct",
          block_number: 10,
          expires_at_unix: 1_900_000_600,
          sources: [],
        },
        destination: {
          quote_id: "0x04",
          request_id: "0x03",
          role: "destination",
          local_chain: 31338,
          remote_chain: 31337,
          input_token: linkAddress,
          output_token: usdcAddress,
          amount_in: "0xde0b6b3a7640000",
          amount_out: "0xaef7a7",
          route: "direct",
          block_number: 11,
          expires_at_unix: 1_900_000_600,
          sources: [],
        },
      },
      destination: { command_id: "0x06", block_number: 74 },
      fill_proof: null,
      origin: null,
      repayment: null,
      failure: null,
    } satisfies CrossChainOrder;

    const trade = toCrossChainTrade(
      order,
      assets.items.map((asset) => toAsset(asset, "Chain A")) as Asset[],
      assets.items.map((asset) => toAsset(asset, "Base")) as Asset[],
    );

    expect(trade.lifecycle.map((stage) => stage.status)).toEqual([
      "quoted",
      "destination fill",
    ]);
    expect(trade.lifecycle.map((stage) => stage.at)).toEqual([null, null]);
    expect(trade.settledAt).toBeNull();
  });
});
