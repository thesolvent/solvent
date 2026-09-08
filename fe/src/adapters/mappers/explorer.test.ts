import { describe, expect, it } from "vitest";

import detail from "@/data/fixtures/trade-detail.json";
import trades from "@/data/fixtures/trades.json";
import activity from "@/data/fixtures/activity.json";
import { toActivity, toTrade } from "./explorer";

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
});
