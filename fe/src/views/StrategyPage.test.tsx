import { fireEvent, screen } from "@testing-library/react";
import { Link, Route } from "react-router-dom";
import { describe, expect, it } from "vitest";

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
