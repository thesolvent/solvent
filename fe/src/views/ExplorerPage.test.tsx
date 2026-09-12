import { focusManager, onlineManager } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { MemoryRouter, useLocation, useNavigate } from "react-router-dom";
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
      expect(screen.getByText("91001")).toBeInTheDocument();
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
      expect(screen.getByText("91003")).toBeInTheDocument();
    } finally {
      view.unmount();
      focusManager.setFocused(undefined);
      onlineManager.setOnline(true);
      vi.useRealTimers();
    }
  });

  it('reports the block height\'s real refresh state instead of a permanent "live"', async () => {
    const healthy = renderWithServices(<ExplorerPage />, {
      ...common,
      explorer: {
        trades: vi.fn().mockResolvedValue({ items: [] }),
        stats: vi.fn().mockResolvedValue(toStats(stats)),
      },
    });
    expect(await screen.findByText(/^Updated ·/)).toBeInTheDocument();
    healthy.unmount();

    const stalled = renderWithServices(<ExplorerPage />, {
      ...common,
      explorer: {
        trades: vi.fn().mockResolvedValue({ items: [] }),
        stats: vi.fn().mockRejectedValue(new Error("temporarily unavailable")),
      },
    });
    try {
      expect(await screen.findByText("Refresh delayed")).toBeInTheDocument();
      expect(screen.queryByText(/^Updated/)).not.toBeInTheDocument();
    } finally {
      stalled.unmount();
    }
  });

  it("holds the list's shape while the first page loads", async () => {
    const view = renderWithServices(<ExplorerPage />, {
      ...common,
      explorer: {
        trades: vi.fn().mockReturnValue(new Promise(() => {})),
        stats: vi.fn().mockResolvedValue(toStats(stats)),
      },
    });
    try {
      expect(
        view.container.querySelectorAll('[data-placeholder="row"]'),
      ).toHaveLength(10);
      expect(screen.getByRole("status", { name: "" })).toHaveTextContent(
        "Loading trades…",
      );
    } finally {
      view.unmount();
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

/** Reads back the address the page wrote, and steps history, so both directions are observable. */
function UrlProbe() {
  const location = useLocation();
  const navigate = useNavigate();
  return (
    <>
      <output aria-label="Current URL">
        {location.pathname}
        {location.search}
      </output>
      <button onClick={() => navigate(-1)}>Back</button>
    </>
  );
}

describe("Explorer view addressing", () => {
  const explorerStubs = {
    ...common,
    explorer: {
      trades: vi.fn().mockResolvedValue({ items: [] }),
      activity: vi.fn().mockResolvedValue({ items: [] }),
      stats: vi.fn().mockResolvedValue(toStats(stats)),
    },
  };

  it("opens the tab and filter named in the URL", async () => {
    const activityRead = vi.fn().mockResolvedValue({ items: [] });
    renderWithServices(
      <ExplorerPage />,
      {
        ...explorerStubs,
        explorer: { ...explorerStubs.explorer, activity: activityRead },
      },
      "/explorer?tab=activity&type=pull&entity=maker",
    );
    expect(screen.getByText("Protocol activity")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /pull/ })).toBeInTheDocument();
    await waitFor(() =>
      expect(activityRead).toHaveBeenCalledWith(
        { kind: "pull", entity: "Maker" },
        undefined,
      ),
    );
  });

  it("writes the tab and filter into the URL, and Back restores the previous view", async () => {
    renderWithServices(
      <>
        <ExplorerPage />
        <UrlProbe />
      </>,
      explorerStubs,
      "/explorer",
    );
    expect(screen.getByLabelText("Current URL")).toHaveTextContent("/explorer");

    fireEvent.click(screen.getByRole("button", { name: "Activity" }));
    expect(screen.getByLabelText("Current URL")).toHaveTextContent(
      "/explorer?tab=activity",
    );

    fireEvent.click(screen.getByRole("button", { name: "All types" }));
    fireEvent.click(screen.getByRole("button", { name: "dock" }));
    expect(screen.getByLabelText("Current URL")).toHaveTextContent(
      "/explorer?tab=activity&type=dock",
    );

    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    await waitFor(() =>
      expect(screen.getByLabelText("Current URL")).toHaveTextContent(
        "/explorer?tab=activity",
      ),
    );
    expect(
      screen.getByRole("button", { name: "All types" }),
    ).toBeInTheDocument();
  });

  it("keeps the unfiltered Trades view as the bare address", async () => {
    const trades = vi.fn().mockResolvedValue({ items: [] });
    renderWithServices(
      <ExplorerPage />,
      { ...explorerStubs, explorer: { ...explorerStubs.explorer, trades } },
      "/explorer",
    );
    await waitFor(() => expect(trades).toHaveBeenCalledWith({}, undefined));
    expect(
      screen.getByRole("button", { name: "All status" }),
    ).toBeInTheDocument();
  });
});
