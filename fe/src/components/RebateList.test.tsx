import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { Asset } from "@/data";
import type { RebateRecord } from "@/data/rebates";
import { RebateList } from "./RebateList";

const assets: Asset[] = [
  {
    chainId: 31_337,
    address: "0x0000000000000000000000000000000000000001",
    symbol: "WETH",
    name: "Wrapped Ether",
    decimals: 18,
    price: 2_500,
    change: "+0.00%",
    tags: ["Majors"],
    net: "Ethereum",
    pairs: ["WETH/USDC"],
  },
  {
    chainId: 31_337,
    address: "0x0000000000000000000000000000000000000002",
    symbol: "USDC",
    name: "USD Coin",
    decimals: 6,
    price: 1,
    change: "+0.00%",
    tags: ["Stables"],
    net: "Ethereum",
    pairs: ["WETH/USDC"],
  },
];

const rebate: RebateRecord = {
  id: "rebate-1",
  status: "ready",
  maker: "0x0000000000000000000000000000000000000003",
  strategyHash:
    "0x0000000000000000000000000000000000000000000000000000000000000003",
  tokenIn: assets[0].address,
  tokenOut: assets[1].address,
  amountIn: 1_000_000_000_000_000_000n,
  amountOut: 2_500_000_000n,
  makerRebate: 1_000_000_000_000_000n,
  executorProfit: 500_000_000_000_000n,
  deviationBps: 50,
  deadlineBlock: 10,
  publishedAt: 1_900_000_000,
  executedAt: null,
  transactionHash: null,
  originTradeId: "first-trade",
};

describe("RebateList", () => {
  it("opens the originating trade without intercepting the Earn action", () => {
    const onOpenTrade = vi.fn();
    const onExecute = vi.fn();
    render(
      <RebateList
        assets={assets}
        pages={[[rebate]]}
        pending={false}
        fetching={false}
        error={false}
        hasMore={false}
        onRetry={vi.fn()}
        onLoadMore={vi.fn().mockResolvedValue(false)}
        currentBlock={1}
        action={{ label: "Earn", onExecute }}
        onOpenTrade={onOpenTrade}
      />,
    );

    const row = screen.getByRole("link", {
      name: "View originating trade first-trade",
    });
    fireEvent.click(row);
    fireEvent.keyDown(row, { key: "Enter" });
    expect(onOpenTrade).toHaveBeenNthCalledWith(1, "first-trade");
    expect(onOpenTrade).toHaveBeenNthCalledWith(2, "first-trade");

    fireEvent.click(screen.getByRole("button", { name: "Earn" }));
    expect(onExecute).toHaveBeenCalledWith("rebate-1");
    expect(onOpenTrade).toHaveBeenCalledTimes(2);
  });
});
