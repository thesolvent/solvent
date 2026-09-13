import { focusManager, onlineManager } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { MemoryRouter, useLocation } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import detail from "@/data/fixtures/trade-detail.json";
import activity from "@/data/fixtures/activity.json";
import stats from "@/data/fixtures/stats.json";
import {
  toActivity,
  toStats,
  toTrade,
  toUniswapXFeed,
} from "@/adapters/mappers/explorer";
import type { RebateRecord } from "@/data/rebates";
import { useAppStore } from "@/store";
import { INITIAL_STATE } from "@/state";
import { fakeServices, renderWithServices } from "@/test/harness";
import { ServicesProvider } from "@/services/ServicesProvider";
import { AppProvider } from "@/AppProvider";
import { ExplorerPage } from "./ExplorerPage";

vi.mock("@privy-io/react-auth", () => ({
  usePrivy: () => ({ connectOrCreateWallet: vi.fn() }),
}));

const trade = toTrade(detail);
const uniswapXFeedOrder = toUniswapXFeed({
  order_hash:
    "0x97b5a5c8bbc252d7d1b4a44f7ed81947dd87c9102b98bfcc19d68a807634ff04",
  source_chain_id: 1,
  token_in: {
    address: "0xC02aaA39b223FE8D0A0E5C4F27eAD9083C756Cc2",
    symbol: "WETH",
    decimals: 18,
  },
  token_out: {
    address: "0xA0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
    symbol: "USDC",
    decimals: 6,
  },
  amount_in: "1250000000000000000",
  required_out: "3700000000",
  market_out_per_in_q18: "3000000000000000000000",
  simulated_amount_out: "3750000000",
  simulated_batch_id: 8,
  observed_at: 1_900_000_000,
  last_seen_at: 1_900_000_002,
});
const common = {
  pools: { list: vi.fn().mockResolvedValue([]) },
  system: {
    config: vi
      .fn()
      .mockResolvedValue({ block_explorer_url: "https://scan.example" }),
  },
};

function LocationPath() {
  const { pathname } = useLocation();
  return <output data-testid="location-path">{pathname}</output>;
}

beforeEach(() => useAppStore.setState(INITIAL_STATE));

describe("Explorer live lists", () => {
  it("refreshes visible trades and stats, keeps cached rows on failure, and resumes on focus and reconnect", async () => {
    vi.useFakeTimers();
    let generation = 0;
    let unavailable = false;
    const trades = vi.fn().mockImplementation(async () => {
      if (unavailable) throw new Error("temporarily unavailable");
      return { items: [{ ...trade, id: `trade-${generation}` }] };
    });
    const readStats = vi.fn().mockImplementation(async () => ({
      ...toStats(stats),
      blockHeight: 91000 + generation,
    }));
    const pools = vi.fn().mockResolvedValue([]);
    const view = render(
      <ServicesProvider
        services={fakeServices({
          ...common,
          pools: { list: pools },
          explorer: { trades, stats: readStats },
        })}
      >
        <MemoryRouter>
          <AppProvider>
            <ExplorerPage />
          </AppProvider>
        </MemoryRouter>
      </ServicesProvider>,
    );
    try {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1);
      });
      expect(
        screen.getByRole("link", { name: "Open trade trade-0" }),
      ).toBeInTheDocument();
      generation = 1;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5_002);
      });
      expect(
        screen.getByRole("link", { name: "Open trade trade-1" }),
      ).toBeInTheDocument();
      expect(screen.getByText("91,001")).toBeInTheDocument();
      expect(pools).toHaveBeenCalledTimes(2);

      unavailable = true;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(6_002);
      });
      expect(
        screen.getByRole("link", { name: "Open trade trade-1" }),
      ).toBeInTheDocument();
      expect(screen.getByRole("alert")).toHaveTextContent(
        "Couldn’t refresh trades.",
      );

      unavailable = false;
      generation = 2;
      await act(async () => {
        focusManager.setFocused(false);
        await vi.advanceTimersByTimeAsync(10_000);
      });
      expect(
        screen.queryByRole("link", { name: "Open trade trade-2" }),
      ).not.toBeInTheDocument();
      await act(async () => {
        focusManager.setFocused(true);
        await vi.advanceTimersByTimeAsync(2);
      });
      expect(
        screen.getByRole("link", { name: "Open trade trade-2" }),
      ).toBeInTheDocument();
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
      generation = 3;
      await act(async () => {
        onlineManager.setOnline(false);
        onlineManager.setOnline(true);
        await vi.advanceTimersByTimeAsync(2);
      });
      expect(
        screen.getByRole("link", { name: "Open trade trade-3" }),
      ).toBeInTheDocument();
      expect(screen.getByText("91,003")).toBeInTheDocument();
    } finally {
      view.unmount();
      focusManager.setFocused(undefined);
      onlineManager.setOnline(true);
      vi.useRealTimers();
    }
  });

  it("refreshes the activity feed as newly indexed events arrive", async () => {
    vi.useFakeTimers();
    const readActivity = vi
      .fn()
      .mockResolvedValueOnce({ items: [] })
      .mockResolvedValue({ items: activity.items.map(toActivity) });
    const view = renderWithServices(<ExplorerPage />, {
      ...common,
      explorer: {
        trades: vi.fn().mockResolvedValue({ items: [] }),
        activity: readActivity,
        stats: vi.fn().mockResolvedValue(toStats(stats)),
      },
    });
    try {
      fireEvent.click(screen.getByRole("button", { name: "Activity" }));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1);
      });
      expect(
        screen.getByText("No activity matches these filters."),
      ).toBeInTheDocument();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5_002);
      });
      const links = screen.getAllByRole("link", {
        name: /View .* transaction on chain explorer/,
      });
      expect(links).toHaveLength(2);
      expect(screen.getAllByText("position")).toHaveLength(2);
      expect(screen.getAllByText("transaction")).toHaveLength(2);
      expect(links[0]).toHaveAttribute(
        "href",
        `https://scan.example/tx/${activity.items[0].tx_hash}`,
      );
      expect(readActivity).toHaveBeenCalledTimes(2);
    } finally {
      view.unmount();
      vi.useRealTimers();
    }
  });

  it("advances server cursors and resets pagination when a filter changes", async () => {
    const trades = vi.fn().mockImplementation(async (filter, cursor) => ({
      items: [
        {
          ...trade,
          id: filter.status ? "filtered" : cursor ? "second" : trade.id,
        },
      ],
      nextCursor: cursor || filter.status ? undefined : "older",
    }));
    renderWithServices(<ExplorerPage />, {
      ...common,
      explorer: { trades, stats: vi.fn().mockResolvedValue(toStats(stats)) },
    });
    expect(
      await screen.findByRole("link", { name: `Open trade ${trade.id}` }),
    ).toHaveAttribute("href", `/explorer/trades/${trade.id}`);
    fireEvent.click(screen.getByRole("button", { name: "Next →" }));
    expect(
      await screen.findByRole("link", { name: "Open trade second" }),
    ).toBeInTheDocument();
    expect(trades).toHaveBeenLastCalledWith({}, "older");
    fireEvent.click(screen.getByRole("button", { name: "All status" }));
    fireEvent.click(screen.getByRole("button", { name: "failed" }));
    expect(
      await screen.findByRole("link", { name: "Open trade filtered" }),
    ).toBeInTheDocument();
    expect(trades).toHaveBeenLastCalledWith({ status: "failed" }, undefined);
    expect(screen.getByRole("button", { name: "← Prev" })).toBeDisabled();
  });

  it("continues a sparse activity page and preserves events sharing an explorer transaction", async () => {
    const readActivity = vi
      .fn()
      .mockImplementation(async (_filter, cursor) =>
        cursor
          ? { items: activity.items.map(toActivity), nextCursor: undefined }
          : { items: [], nextCursor: "older-events" },
      );
    renderWithServices(<ExplorerPage />, {
      ...common,
      explorer: {
        trades: vi.fn().mockResolvedValue({ items: [], nextCursor: undefined }),
        activity: readActivity,
        stats: vi.fn().mockResolvedValue(toStats(stats)),
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Activity" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Next →" })).toBeEnabled(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Next →" }));
    const links = await screen.findAllByRole("link", {
      name: /View .* transaction on chain explorer/,
    });
    expect(links).toHaveLength(2);
    expect(links[0]).toHaveAttribute(
      "href",
      `https://scan.example/tx/${activity.items[0].tx_hash}`,
    );
    expect(readActivity).toHaveBeenLastCalledWith({}, "older-events");
    expect(screen.getByRole("button", { name: "Next →" })).toBeDisabled();
  });

  it("shows a paginated, read-only UniswapX simulation feed", async () => {
    const uniswapxFeed = vi.fn().mockImplementation(async (cursor) => ({
      items: [
        {
          ...uniswapXFeedOrder,
          orderHash: cursor
            ? "0x3ff0f1e7b8e08a96c72adc91d60ae8063f8145d75d3a5645ee89c8136c6cb205"
            : uniswapXFeedOrder.orderHash,
        },
      ],
      nextCursor: cursor ? undefined : "older-feed",
    }));
    renderWithServices(<ExplorerPage />, {
      ...common,
      explorer: {
        trades: vi.fn().mockResolvedValue({ items: [] }),
        uniswapxFeed,
        stats: vi.fn().mockResolvedValue(toStats(stats)),
      },
    });

    fireEvent.click(screen.getByRole("button", { name: "UniswapX Feed" }));

    expect(await screen.findByText("UniswapX feed")).toBeInTheDocument();
    expect(await screen.findByText("UniswapX · Ethereum")).toBeInTheDocument();
    expect(screen.getByText("simulated")).toBeInTheDocument();
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Next →" })).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "Next →" }));

    await waitFor(() =>
      expect(uniswapxFeed).toHaveBeenLastCalledWith("older-feed"),
    );
    expect(screen.getByRole("button", { name: "Next →" })).toBeDisabled();
  });

  it("opens the first trade that accrued a rebate", async () => {
    useAppStore.setState({ ...INITIAL_STATE, xpTab: "Rebates" });
    const rebate: RebateRecord = {
      id: "rebate-1",
      status: "ready",
      maker: "0x0000000000000000000000000000000000000001",
      strategyHash:
        "0x0000000000000000000000000000000000000000000000000000000000000001",
      tokenIn: "0x0000000000000000000000000000000000000002",
      tokenOut: "0x0000000000000000000000000000000000000003",
      amountIn: 1_000n,
      amountOut: 2_000n,
      makerRebate: 10n,
      executorProfit: 1n,
      deviationBps: 50,
      deadlineBlock: null,
      publishedAt: 1_900_000_000,
      executedAt: null,
      transactionHash: null,
      originTradeId: "first-trade",
    };
    renderWithServices(
      <>
        <ExplorerPage />
        <LocationPath />
      </>,
      {
        ...common,
        assets: { list: vi.fn().mockResolvedValue([]) },
        rebates: {
          list: vi.fn().mockResolvedValue({ items: [rebate] }),
        },
        explorer: { stats: vi.fn().mockResolvedValue(toStats(stats)) },
      },
      "/explorer",
    );

    fireEvent.click(
      await screen.findByRole("link", {
        name: "View originating trade first-trade",
      }),
    );

    expect(screen.getByTestId("location-path")).toHaveTextContent(
      "/explorer/trades/first-trade",
    );
  });
});
