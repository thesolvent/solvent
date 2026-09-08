import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { Route, useParams } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Asset, Quote, SubmittedSwap } from "@/data";
import { INITIAL_STATE, useAppActions } from "@/state";
import { useAppStore } from "@/store";
import { renderWithServices } from "@/test/harness";
import { TransitionRoutes } from "@/components/TransitionRoutes";
import * as swapService from "@/services/swap";
import { SwapPage } from "@/views/SwapPage";

vi.mock("wagmi", async (original) => ({
  ...(await original<typeof import("wagmi")>()),
  useAccount: () => ({
    isConnected: true,
    address: "0x1111111111111111111111111111111111111111",
    chainId: 31337,
  }),
  useClient: () => ({}),
  useConnectorClient: () => ({ data: {} }),
}));

beforeEach(() => useAppStore.setState(INITIAL_STATE));

function TradeDestination() {
  const { tradeId } = useParams();
  return <p>Trade destination {tradeId}</p>;
}

function LeaveSwap() {
  const { go } = useAppActions();
  return (
    <>
      <button onClick={go("Pools")}>Leave swap</button>
      <button onClick={go("Swap")}>Stay on swap</button>
    </>
  );
}

function swapRoutes() {
  return (
    <TransitionRoutes>
      <Route
        path="/swap"
        element={
          <>
            <SwapPage />
            <LeaveSwap />
          </>
        }
      />
      <Route path="/explorer/trades/:tradeId" element={<TradeDestination />} />
      <Route path="/pools" element={<p>Pools destination</p>} />
    </TransitionRoutes>
  );
}

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
  amountOutRaw: 2_477_852_376n,
  expiresAt: Date.now() + 30_000,
};

const ASSETS = [
  asset("WETH", 18, 2495, ["WETH/USDC"]),
  asset("USDC", 6, 1, ["WETH/USDC"]),
];

describe("SwapPage", () => {
  it.each([false, true])(
    "transitions to the returned trade only after the backend accepts the order (same-page navigation: %s)",
    async (samePageNavigation) => {
      let accept!: (result: SubmittedSwap) => void;
      const submit = vi.fn().mockReturnValue(
        new Promise<SubmittedSwap>((resolve) => {
          accept = resolve;
        }),
      );
      const view = renderWithServices(
        swapRoutes(),
        {
          assets: { list: vi.fn().mockResolvedValue(ASSETS) },
          swap: {
            quote: vi.fn().mockResolvedValue(QUOTE),
            createIntent: () => ({ submit }),
          },
        },
        "/swap",
        "browser",
      );
      try {
        await screen.findByText("2,477");
        vi.useFakeTimers();
        fireEvent.click(
          screen.getAllByRole("button", { name: "Swap" }).at(-1)!,
        );
        await act(async () => {
          await vi.advanceTimersByTimeAsync(1000);
        });
        expect(submit).toHaveBeenCalledOnce();
        expect(
          screen.queryByTestId("route-transition"),
        ).not.toBeInTheDocument();
        expect(screen.queryByText(/Trade destination/)).not.toBeInTheDocument();
        if (samePageNavigation)
          fireEvent.click(screen.getByRole("button", { name: "Stay on swap" }));
        await act(async () => {
          accept({ tradeId: "accepted-order-123", status: "submitted" });
        });
        expect(screen.getByTestId("route-transition")).toBeInTheDocument();
        expect(screen.queryByText(/Trade destination/)).not.toBeInTheDocument();
        await act(async () => {
          await vi.advanceTimersByTimeAsync(210);
        });
        expect(
          screen.getByText("Trade destination accepted-order-123"),
        ).toBeInTheDocument();
        expect(screen.getByTestId("route-transition")).toBeInTheDocument();
        await act(async () => {
          await vi.advanceTimersByTimeAsync(310);
        });
        expect(
          screen.queryByTestId("route-transition"),
        ).not.toBeInTheDocument();
        expect(submit).toHaveBeenCalledOnce();
      } finally {
        view.unmount();
        vi.useRealTimers();
      }
    },
  );

  it("keeps a rejected submission on Swap without a route transition", async () => {
    const submit = vi.fn().mockRejectedValue(new Error("Submission failed"));
    renderWithServices(
      swapRoutes(),
      {
        assets: { list: vi.fn().mockResolvedValue(ASSETS) },
        swap: {
          quote: vi.fn().mockResolvedValue(QUOTE),
          createIntent: () => ({ submit }),
        },
      },
      "/swap",
      "browser",
    );
    await screen.findByText("2,477");
    fireEvent.click(screen.getAllByRole("button", { name: "Swap" }).at(-1)!);
    expect(
      await screen.findByRole("button", {
        name: "Could not submit the swap — try again",
      }),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("route-transition")).not.toBeInTheDocument();
    expect(screen.queryByText(/Trade destination/)).not.toBeInTheDocument();
  });

  it.each([
    { phase: "during the sweep", delay: 0 },
    { phase: "after unmount", delay: 210 },
  ])("preserves a requested departure $phase", async ({ delay }) => {
    let accept!: (result: SubmittedSwap) => void;
    const submit = vi.fn().mockReturnValue(
      new Promise<SubmittedSwap>((resolve) => {
        accept = resolve;
      }),
    );
    const view = renderWithServices(
      swapRoutes(),
      {
        assets: { list: vi.fn().mockResolvedValue(ASSETS) },
        swap: {
          quote: vi.fn().mockResolvedValue(QUOTE),
          createIntent: () => ({ submit }),
        },
      },
      "/swap",
      "browser",
    );
    try {
      await screen.findByText("2,477");
      fireEvent.click(screen.getAllByRole("button", { name: "Swap" }).at(-1)!);
      await waitFor(() => expect(submit).toHaveBeenCalledOnce());
      vi.useFakeTimers();
      fireEvent.click(screen.getByRole("button", { name: "Leave swap" }));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(delay);
      });
      await act(async () => {
        accept({ tradeId: "late-order", status: "submitted" });
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(520);
      });
      expect(screen.queryByTestId("route-transition")).not.toBeInTheDocument();
      expect(screen.getByText("Pools destination")).toBeInTheDocument();
    } finally {
      view.unmount();
      vi.useRealTimers();
    }
  });

  it("preserves a browser departure before React renders its location", async () => {
    let accept!: (result: SubmittedSwap) => void;
    const submission = vi
      .spyOn(swapService, "useSubmitSwap")
      .mockImplementation((_form, onSubmitted) => {
        accept = onSubmitted!;
        return {
          send: vi.fn(),
          submitting: true,
          result: undefined,
          problem: undefined,
        };
      });
    const view = renderWithServices(
      swapRoutes(),
      {
        assets: { list: vi.fn().mockResolvedValue(ASSETS) },
        swap: { quote: vi.fn().mockResolvedValue(QUOTE) },
      },
      "/swap",
      "browser",
    );
    try {
      vi.useFakeTimers();
      await act(async () => {
        screen.getByRole("button", { name: "Leave swap" }).click();
        await Promise.resolve();
        expect(window.location.pathname).toBe("/pools");
        expect(
          screen.queryByTestId("route-transition"),
        ).not.toBeInTheDocument();
        accept({ tradeId: "late-order", status: "submitted" });
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(520);
      });
      expect(screen.getByText("Pools destination")).toBeInTheDocument();
    } finally {
      view.unmount();
      submission.mockRestore();
      vi.useRealTimers();
    }
  });

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
