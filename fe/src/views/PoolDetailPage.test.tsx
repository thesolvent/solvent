import { act, fireEvent, screen, within } from "@testing-library/react";
import { Route, Routes, useParams } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";

import type { Pool } from "@/data";
import detail from "@/data/fixtures/trade-detail.json";
import { toTrade } from "@/adapters/mappers/explorer";
import { renderWithServices } from "@/test/harness";
import { PoolDetailPage } from "./PoolDetailPage";

const pool: Pool = {
  pair: "DAI / USDC",
  type: "Stable",
  venue: "Aqua core",
  range: "—",
  tvl: "—",
  vol: "—",
  fills: "—",
  fee: "0.05%",
  apr: "—",
  ref: {
    base: detail.input.token.address,
    quote: detail.output.token.address,
    baseDecimals: detail.input.token.decimals,
    quoteDecimals: detail.output.token.decimals,
  },
};

function TradeDestination() {
  const { tradeId } = useParams();
  return <p>Trade destination {tradeId}</p>;
}

describe("Pool settlements", () => {
  it("waits for the pool pair, loads only its confirmed trades, and links each actual direction to its detail", async () => {
    let resolvePools: ((pools: Pool[]) => void) | undefined;
    const pools = new Promise<Pool[]>((resolve) => {
      resolvePools = resolve;
    });
    const trade = toTrade(detail);
    const trades = vi.fn().mockResolvedValue({
      items: [
        trade,
        {
          ...trade,
          id: "reverse-trade",
          input: trade.output,
          output: trade.input,
        },
      ],
    });
    renderWithServices(
      <Routes>
        <Route path="/pools/:pair" element={<PoolDetailPage />} />
        <Route
          path="/explorer/trades/:tradeId"
          element={<TradeDestination />}
        />
      </Routes>,
      {
        pools: {
          list: vi.fn().mockReturnValue(pools),
          detail: vi.fn().mockResolvedValue({ pool, makers: [] }),
          depth: vi.fn().mockResolvedValue({ levels: [] }),
        },
        explorer: { trades },
      },
      "/pools/dai-usdc",
    );
    expect(trades).not.toHaveBeenCalled();
    await act(async () => {
      resolvePools?.([pool]);
    });
    const forward = await screen.findByRole("link", {
      name: `Open trade ${trade.id}`,
    });
    expect(trades).toHaveBeenCalledWith(
      {
        status: "confirmed",
        base: detail.input.token.address,
        quote: detail.output.token.address,
      },
      undefined,
    );
    expect(forward).toHaveAttribute("href", `/explorer/trades/${trade.id}`);
    expect(within(forward).getByText("5,000 DAI")).toBeInTheDocument();
    expect(within(forward).getByText("4,946.047017 USDC")).toBeInTheDocument();
    expect(forward.querySelector("time")).toHaveAttribute(
      "datetime",
      new Date(detail.settled_at * 1000).toISOString(),
    );
    const reverse = screen.getByRole("link", {
      name: "Open trade reverse-trade",
    });
    expect(reverse.textContent).toMatch(/4,946.047017 USDC.*5,000 DAI/);
    fireEvent.click(reverse);
    expect(
      await screen.findByText("Trade destination reverse-trade"),
    ).toBeInTheDocument();
  });
});
