import { screen, waitFor } from "@testing-library/react";
import { Route, Routes } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";

import type { MakerDashboard } from "@/data/makers";
import { renderWithServices, type Stubs } from "@/test/harness";

import { MakersPage } from "./MakersPage";

const MAKER = "0x3c44cdddb6a900fa2b585dd299e03d12fa4293bc";

const DASHBOARD: MakerDashboard = {
  address: MAKER,
  activePositions: 0,
  sharedLiquidityUsd: 0,
  windowDays: 7,
  volumeUsd: 0,
  feesUsd: 0,
  walletUsd: 0,
  pullableUsd: 0,
  coverage: null,
  liquidityChangePct: null,
  volumeChangePct: null,
  feesChangePct: null,
  fills: 0,
  pairFills: 0,
  sharePct: null,
  latencyMs: null,
  previousLatencyMs: null,
  fillsChangePct: null,
  activity: [],
  insight: null,
};

const ROSTER = [{ address: MAKER, activePositions: 0, sharedLiquidityUsd: 0 }];

const emptyPage = { items: [], nextCursor: undefined };

function makers(stubs: Stubs = {}, route = `/makers/${MAKER}`) {
  return renderWithServices(
    <Routes>
      <Route path="/makers/:maker" element={<MakersPage />} />
    </Routes>,
    {
      pools: { list: vi.fn().mockResolvedValue([]) },
      assets: { list: vi.fn().mockResolvedValue([]) },
      rebates: { list: vi.fn().mockResolvedValue(emptyPage) },
      ...stubs,
      makers: {
        list: vi.fn().mockResolvedValue(ROSTER),
        dashboard: vi.fn().mockResolvedValue(DASHBOARD),
        positions: vi.fn().mockResolvedValue([]),
        inventory: vi.fn().mockResolvedValue([]),
        settlements: vi.fn().mockResolvedValue(emptyPage),
        ...stubs.makers,
      },
    },
    route,
  );
}

describe("a maker that failed to load", () => {
  const failing = {
    makers: {
      list: vi.fn().mockResolvedValue(ROSTER),
      dashboard: vi.fn().mockRejectedValue(new Error("upstream is down")),
      positions: vi.fn().mockRejectedValue(new Error("upstream is down")),
      inventory: vi.fn().mockRejectedValue(new Error("upstream is down")),
      settlements: vi.fn().mockRejectedValue(new Error("upstream is down")),
    },
  };

  it("marks the charts unavailable instead of drawing zeros", async () => {
    makers(failing);

    // `useMakerRead` retries once before it settles on the failure.
    await waitFor(
      () =>
        expect(
          screen.getByText("Couldn’t load fill share."),
        ).toBeInTheDocument(),
      { timeout: 5000 },
    );
    expect(screen.getByText("Couldn’t load fills.")).toBeInTheDocument();
    expect(screen.getByText("Couldn’t load fill latency.")).toBeInTheDocument();
    // A drawn donut would total the fills it does not have.
    expect(screen.queryByText("total")).not.toBeInTheDocument();
  });

  it("does not reprint the failure as an insight", async () => {
    makers(failing);

    await waitFor(
      () =>
        expect(
          screen.getByText("Couldn’t load positions."),
        ).toBeInTheDocument(),
      { timeout: 5000 },
    );
    expect(screen.queryByText("Insight")).not.toBeInTheDocument();
    expect(screen.queryByText(/upstream is down/)).not.toBeInTheDocument();
  });
});

describe("an answered maker with nothing in it", () => {
  it("says the tab is empty rather than leaving it blank", async () => {
    makers();

    expect(
      await screen.findByText("No positions — this maker is not quoting."),
    ).toBeInTheDocument();
  });

  it("still reports its own fill share", async () => {
    makers();

    expect(await screen.findByText("Insight")).toBeInTheDocument();
    expect(
      screen.queryByText("Couldn’t load fill share."),
    ).not.toBeInTheDocument();
  });
});

describe("an address that is not a maker", () => {
  it("says so rather than retrying behind a caption", async () => {
    makers({}, "/makers/0x0000000000000000000000000000000000000dead");

    expect(
      await screen.findByRole("heading", { level: 1, name: "No such page" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to home" })).toHaveAttribute(
      "href",
      "/",
    );
  });
});

describe("chart accessibility", () => {
  it("names each chart and gives its hit targets to the keyboard", async () => {
    makers({
      makers: {
        list: vi.fn().mockResolvedValue(ROSTER),
        dashboard: vi.fn().mockResolvedValue({
          ...DASHBOARD,
          fills: 3,
          pairFills: 12,
          activity: [
            { from: 0, to: 86_400, fills: 3, latencyMs: 400 },
            { from: 86_400, to: 172_800, fills: 1, latencyMs: 900 },
          ],
        }),
        positions: vi.fn().mockResolvedValue([]),
        inventory: vi.fn().mockResolvedValue([]),
        settlements: vi.fn().mockResolvedValue(emptyPage),
      },
    });

    const share = await screen.findByRole("img", { name: /^Fill share:/ });
    expect(share).toBeInTheDocument();
    expect(screen.getByRole("img", { name: /^Fills by/ })).toBeInTheDocument();
    expect(
      screen.getByRole("img", { name: /^Fill latency p50 by/ }),
    ).toBeInTheDocument();

    // Legend rows, bars and latency points are all reachable and self-describing.
    expect(
      screen.getByRole("button", { name: "This maker 25.0%" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /^Jan 1.*3 fills$/ }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /^Jan 2.*900 ms p50$/ }),
    ).toBeInTheDocument();
  });
});
