import { screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { Asset, Quote } from "@/data";
import { renderWithServices } from "@/test/harness";
import { SwapPage } from "@/views/SwapPage";

function asset(
  symbol: string,
  decimals: number,
  price: number,
  pairs: string[] = [],
): Asset {
  return {
    address: `0x${symbol}`,
    symbol,
    name: symbol,
    decimals,
    price,
    change: "+0.00%",
    tags: ["Majors"],
    net: "Ethereum",
    pairs,
  };
}

const QUOTE: Quote = {
  amountOut: "2,477.85",
  amountOutUsd: 2478.11,
  priceImpact: "0.12%",
  makersSourced: 3,
  expiresAt: Date.now() + 30_000,
};

const ASSETS = [
  asset("WETH", 18, 2495, ["WETH/USDC"]),
  asset("USDC", 6, 1, ["WETH/USDC"]),
];

describe("SwapPage", () => {
  it("opens on a pair the deployment serves, not the mock's default", async () => {
    renderWithServices(<SwapPage />, {
      assets: { list: vi.fn().mockResolvedValue(ASSETS) },
      swap: { quote: vi.fn().mockResolvedValue(QUOTE) },
    });

    // The initial state names ETH -> SOL; neither is served here.
    expect(await screen.findAllByText("WETH")).not.toHaveLength(0);
  });

  it("shows the server's price and route, not a mid-price estimate", async () => {
    const quote = vi.fn().mockResolvedValue(QUOTE);
    renderWithServices(<SwapPage />, {
      assets: { list: vi.fn().mockResolvedValue(ASSETS) },
      swap: { quote },
    });

    expect(await screen.findByText("2,477")).toBeInTheDocument();
    expect(await screen.findByText("0.12%")).toBeInTheDocument();
    expect(await screen.findByText("3 makers")).toBeInTheDocument();
  });

  it("puts the server's reason on the button and stops the trade", async () => {
    renderWithServices(<SwapPage />, {
      assets: { list: vi.fn().mockResolvedValue(ASSETS) },
      swap: {
        quote: vi
          .fn()
          .mockRejectedValue(new Error("no route for this pair and size")),
      },
    });

    const button = await screen.findByRole("button", {
      name: "No route for this pair and size",
    });
    expect(button).toBeDisabled();
  });
});
