import { fireEvent, screen, waitFor } from "@testing-library/react";
import { Link, Route } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";

import { TransitionRoutes } from "@/components/TransitionRoutes";
import { renderWithServices } from "@/test/harness";
import { StrategyPage } from "./StrategyPage";

const strategyPath = "/explorer/strategies/test-strategy";

function renderStrategy(route: string) {
  const pending = new Promise<never>(() => {});
  return renderWithServices(
    <TransitionRoutes>
      <Route path="/explorer" element={<p>Explorer destination</p>} />
      <Route
        path="/pools/:pair"
        element={<Link to={strategyPath}>Open strategy</Link>}
      />
      <Route
        path="/explorer/strategies/:strategyHash"
        element={<StrategyPage />}
      />
    </TransitionRoutes>,
    {
      makers: { position: () => pending, history: () => pending },
      explorer: { trades: () => pending },
    },
    route,
    "browser",
  );
}

describe("Strategy back navigation", () => {
  it("scopes a chain-qualified strategy to its destination service", async () => {
    const pending = new Promise<never>(() => {});
    const position = vi.fn(() => pending);
    const trades = vi.fn(() => pending);
    renderWithServices(
      <TransitionRoutes>
        <Route
          path="/explorer/strategies/:strategyHash"
          element={<StrategyPage />}
        />
      </TransitionRoutes>,
      {
        makers: { position, history: () => pending },
        explorer: { trades },
      },
      `${strategyPath}?chain=31338`,
    );

    await waitFor(() => {
      expect(position).toHaveBeenCalledWith("test-strategy", 31338);
      expect(trades).toHaveBeenCalledWith(
        {
          status: "confirmed",
          strategy_hash: "test-strategy",
          chainId: 31338,
        },
        undefined,
      );
    });
  });

  it("replaces a directly opened strategy with Explorer even while data is loading", async () => {
    window.history.pushState(null, "", "/outside-the-app");
    renderStrategy(strategyPath);

    fireEvent.click(screen.getByRole("button", { name: "←" }));

    expect(await screen.findByText("Explorer destination")).toBeVisible();
    expect(window.location.pathname).toBe("/explorer");
    expect(window.history.state.idx).toBe(0);
  });

  it("returns to the previous app route with its query and the green transition", async () => {
    renderStrategy("/pools/dai-usdc?sort=apr");
    fireEvent.click(screen.getByRole("link", { name: "Open strategy" }));
    const back = await screen.findByRole("button", { name: "←" });

    fireEvent.click(back);

    expect(
      await screen.findByRole("link", { name: "Open strategy" }),
    ).toBeVisible();
    expect(screen.getByTestId("route-transition")).toBeInTheDocument();
    expect(window.location.pathname + window.location.search).toBe(
      "/pools/dai-usdc?sort=apr",
    );
  });
});
