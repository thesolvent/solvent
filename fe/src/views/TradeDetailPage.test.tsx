import { act, fireEvent, screen } from "@testing-library/react";
import { Link, Route, Routes } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import { SolventApiError } from "@solvent/sdk/client";
import detail from "@/data/fixtures/trade-detail.json";
import { toTrade } from "@/adapters/mappers/explorer";
import { renderWithServices } from "@/test/harness";
import { TradeDetailPage } from "./TradeDetailPage";

function routes() {
  return (
    <Routes>
      <Route
        path="/explorer"
        element={
          <Link to={`/explorer/trades/${detail.id}`}>Return to trade</Link>
        }
      />
      <Route path="/explorer/trades/:tradeId" element={<TradeDetailPage />} />
    </Routes>
  );
}

describe("Trade detail navigation", () => {
  it("refreshes a submitted intent until it is confirmed, then stops polling", async () => {
    vi.useFakeTimers();
    const read = vi
      .fn()
      .mockResolvedValueOnce(
        toTrade({
          ...detail,
          status: "submitted",
          settled_at: undefined,
          lifecycle: detail.lifecycle.slice(0, -1),
        }),
      )
      .mockResolvedValue(toTrade(detail));
    try {
      const view = renderWithServices(
        routes(),
        {
          explorer: { trade: read },
          system: { config: vi.fn().mockResolvedValue({}) },
        },
        `/explorer/trades/${detail.id}`,
      );
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1);
      });
      expect(screen.getByText(/ → min\. /)).toBeInTheDocument();
      expect(
        screen.getByRole("listitem", { name: "Confirmed: awaiting" }),
      ).toHaveAttribute("aria-current", "step");
      expect(screen.getByRole("status")).toHaveTextContent(
        "Trade submitted. 5 of 6 stages recorded.",
      );
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2_001);
      });
      expect(screen.getByText("In → out")).toBeInTheDocument();
      expect(screen.getByRole("status")).toHaveTextContent(
        "Trade confirmed. 6 of 6 stages recorded.",
      );
      expect(
        screen.getByRole("listitem", { name: "Confirmed: recorded" }),
      ).not.toHaveAttribute("aria-current");
      await act(async () => {
        await vi.advanceTimersByTimeAsync(30_000);
      });
      expect(read).toHaveBeenCalledTimes(2);
      view.unmount();
    } finally {
      vi.useRealTimers();
    }
  });

  it.each(["declined", "failed"])(
    "stops refreshing after a pending trade becomes %s",
    async (status) => {
      vi.useFakeTimers();
      const pending = toTrade({
        ...detail,
        status: "quoted",
        settled_at: undefined,
        lifecycle: detail.lifecycle.slice(0, 2),
      });
      const read = vi
        .fn()
        .mockResolvedValueOnce(pending)
        .mockResolvedValue({
          ...pending,
          status,
        });
      const view = renderWithServices(
        routes(),
        {
          explorer: { trade: read },
          system: { config: vi.fn().mockResolvedValue({}) },
        },
        `/explorer/trades/${detail.id}`,
      );
      try {
        await act(async () => {
          await vi.advanceTimersByTimeAsync(2_002);
        });
        expect(screen.getByRole("status")).toHaveTextContent(
          `Trade ${status}.`,
        );
        expect(
          screen.getByRole("listitem", { name: "Reserved: not reached" }),
        ).not.toHaveAttribute("aria-current");
        await act(async () => {
          await vi.advanceTimersByTimeAsync(30_000);
        });
        expect(read).toHaveBeenCalledTimes(2);
      } finally {
        view.unmount();
        vi.useRealTimers();
      }
    },
  );

  it("keeps the last trade during a refresh failure and recovers on the next poll", async () => {
    vi.useFakeTimers();
    const pending = toTrade({
      ...detail,
      status: "submitted",
      settled_at: undefined,
      lifecycle: detail.lifecycle.slice(0, -1),
    });
    let unavailable = false;
    const read = vi.fn().mockImplementation(async () => {
      if (unavailable) throw new Error("temporarily unavailable");
      return read.mock.calls.length === 1 ? pending : toTrade(detail);
    });
    const view = renderWithServices(
      routes(),
      {
        explorer: { trade: read },
        system: { config: vi.fn().mockResolvedValue({}) },
      },
      `/explorer/trades/${detail.id}`,
    );
    try {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1);
      });
      unavailable = true;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(3_002);
      });
      expect(screen.getByText(/ → min\. /)).toBeInTheDocument();
      expect(screen.getByRole("alert")).toHaveTextContent(
        "Couldn’t refresh this trade.",
      );
      expect(screen.getByText(/Refresh delayed/)).toBeInTheDocument();
      unavailable = false;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2_002);
      });
      expect(screen.getByText("In → out")).toBeInTheDocument();
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    } finally {
      view.unmount();
      vi.useRealTimers();
    }
  });

  it.each([400, 404])(
    "does not retry or poll a permanent %s trade error",
    async (status) => {
      vi.useFakeTimers();
      const read = vi
        .fn()
        .mockRejectedValue(new SolventApiError(status, "trade not found"));
      const view = renderWithServices(
        routes(),
        {
          explorer: { trade: read },
          system: { config: vi.fn().mockResolvedValue({}) },
        },
        "/explorer/trades/missing",
      );
      try {
        await act(async () => {
          await vi.advanceTimersByTimeAsync(1);
        });
        expect(screen.getByRole("alert")).toHaveTextContent("Trade not found.");
        expect(
          screen.queryByText(`Trade #${detail.id}`),
        ).not.toBeInTheDocument();
        expect(
          screen.getByRole("link", { name: "Back to Explorer" }),
        ).toHaveAttribute("href", "/explorer");
        await act(async () => {
          await vi.advanceTimersByTimeAsync(30_000);
        });
        expect(read).toHaveBeenCalledTimes(1);
      } finally {
        view.unmount();
        vi.useRealTimers();
      }
    },
  );

  it("loads a direct URL and returns through both breadcrumb and Back", async () => {
    const read = vi.fn().mockResolvedValue(toTrade(detail));
    renderWithServices(
      routes(),
      {
        explorer: { trade: read },
        system: {
          config: vi
            .fn()
            .mockResolvedValue({ block_explorer_url: "https://scan.example" }),
        },
      },
      `/explorer/trades/${detail.id}`,
    );
    expect(
      await screen.findByRole("link", { name: "View transaction" }),
    ).toHaveAttribute("href", `https://scan.example/tx/${detail.tx_hash}`);
    expect(read).toHaveBeenCalledWith(detail.id);
    fireEvent.click(screen.getByRole("button", { name: "Explorer" }));
    fireEvent.click(
      await screen.findByRole("link", { name: "Return to trade" }),
    );
    fireEvent.click(
      await screen.findByRole("link", { name: "Back to Explorer" }),
    );
    expect(
      await screen.findByRole("link", { name: "Return to trade" }),
    ).toBeInTheDocument();
  });

  it("identifies both tokens and networks in a SolventX trade summary", async () => {
    const trade = {
      ...toTrade(detail),
      flow: "cross-chain" as const,
      input: {
        symbol: "LINK",
        display: "1",
        net: "Chain A",
      },
      output: {
        symbol: "USDC",
        display: "11.466663",
        net: "Base",
      },
    };
    renderWithServices(
      routes(),
      {
        explorer: { trade: vi.fn().mockResolvedValue(trade) },
        system: { config: vi.fn().mockResolvedValue({}) },
      },
      `/explorer/trades/${detail.id}`,
    );

    expect(
      await screen.findByLabelText("LINK token on Chain A"),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("USDC token on Base")).toBeInTheDocument();
    expect(
      screen.queryByText("1 LINK → 11.466663 USDC"),
    ).not.toBeInTheDocument();
  });
});
