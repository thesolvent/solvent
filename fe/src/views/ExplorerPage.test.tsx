import { focusManager, onlineManager } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import detail from "@/data/fixtures/trade-detail.json";
import activity from "@/data/fixtures/activity.json";
import stats from "@/data/fixtures/stats.json";
import { toActivity, toStats, toTrade } from "@/adapters/mappers/explorer";
import { useAppStore } from "@/store";
import { INITIAL_STATE } from "@/state";
import { fakeServices, renderWithServices } from "@/test/harness";
import { ServicesProvider } from "@/services/ServicesProvider";
import { AppProvider } from "@/AppProvider";
import { ExplorerPage } from "./ExplorerPage";

const trade = toTrade(detail);
const common = {
  pools: { list: vi.fn().mockResolvedValue([]) },
  system: {
    config: vi
      .fn()
      .mockResolvedValue({ block_explorer_url: "https://scan.example" }),
  },
};

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
      expect(
        screen.getAllByRole("link", { name: /View .* transaction/ }),
      ).toHaveLength(2);
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

  it("continues a sparse activity page and preserves events sharing a transaction", async () => {
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
      name: /View .* transaction/,
    });
    expect(links).toHaveLength(2);
    expect(links[0]).toHaveAttribute(
      "href",
      `/explorer/strategies/${activity.items[0].strategy_hash}`,
    );
    expect(readActivity).toHaveBeenLastCalledWith({}, "older-events");
    expect(screen.getByRole("button", { name: "Next →" })).toBeDisabled();
  });
});
