import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  Link,
  MemoryRouter,
  Navigate,
  Route,
  useLocation,
  useNavigate,
} from "react-router-dom";
import { AppProvider } from "@/AppProvider";
import { INITIAL_STATE, useAppActions } from "@/state";
import { useAppStore } from "@/store";
import { TransitionRoutes } from "./TransitionRoutes";

function Controls() {
  const { navTo } = useAppActions();
  const navigate = useNavigate();
  const location = useLocation();
  return (
    <>
      <button onClick={() => navTo("Pools")}>Pools via header</button>
      <button onClick={() => navTo("Explorer")}>Explorer via header</button>
      <output aria-label="Current URL">
        {location.pathname}
        {location.search}
      </output>
      <Link to="/trades/one">Trade one</Link>
      <button
        onClick={() => navigate("/trades/two?source=pools", { replace: true })}
      >
        Trade two
      </button>
      <button onClick={() => navigate(-1)}>Back</button>
      <button onClick={() => navigate(1)}>Forward</button>
    </>
  );
}

function renderRoutes(initial = "/pools") {
  return render(
    <MemoryRouter initialEntries={[initial]}>
      <AppProvider>
        <Controls />
        <TransitionRoutes>
          <Route path="/pools" element={<h1>Pools page</h1>} />
          <Route path="/explorer" element={<h1>Explorer page</h1>} />
          <Route path="/trades/:id" element={<Trade />} />
          <Route path="*" element={<Navigate to="/pools" replace />} />
        </TransitionRoutes>
      </AppProvider>
    </MemoryRouter>,
  );
}

function Trade() {
  const location = useLocation();
  return (
    <h1>
      Trade page {location.pathname}
      {location.search}
    </h1>
  );
}

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

beforeEach(() => {
  useAppStore.setState(INITIAL_STATE);
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("TransitionRoutes", () => {
  it("preserves subview selections when traversing a header-created history entry", async () => {
    renderRoutes();
    fireEvent.click(
      screen.getByRole("button", { name: "Explorer via header" }),
    );
    await advance(520);
    const strategy = { maker: "maker", curve: "pegged", pair: "DAI/USDC" };
    act(() => useAppStore.setState({ xpStrat: strategy }));
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    await advance(520);
    fireEvent.click(screen.getByRole("button", { name: "Forward" }));
    await advance(520);
    expect(useAppStore.getState().xpStrat).toEqual(strategy);
  });

  it("retains outgoing subview selections until header navigation is revealed", async () => {
    const strategy = { maker: "maker", curve: "pegged", pair: "DAI/USDC" };
    useAppStore.setState({ xpStrat: strategy });
    renderRoutes("/trades/one");
    fireEvent.click(screen.getByRole("button", { name: "Pools via header" }));
    expect(useAppStore.getState().xpStrat).toEqual(strategy);
    await advance(210);
    expect(useAppStore.getState().xpStrat).toBeNull();
    expect(
      screen.getByRole("heading", { name: "Pools page" }),
    ).toBeInTheDocument();
  });

  it("shows the destination immediately when reduced motion is requested", () => {
    vi.stubGlobal("matchMedia", () => ({ matches: true }));
    renderRoutes();
    fireEvent.click(screen.getByRole("link", { name: "Trade one" }));
    expect(
      screen.getByRole("heading", { name: "Trade page /trades/one" }),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("route-transition")).not.toBeInTheDocument();
  });

  it("covers link and browser history navigation while retaining the outgoing route", async () => {
    renderRoutes();
    expect(screen.queryByTestId("route-transition")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("link", { name: "Trade one" }));
    expect(screen.getByLabelText("Current URL")).toHaveTextContent(
      "/trades/one",
    );
    expect(screen.getByTestId("route-transition")).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "Pools page" }),
    ).toBeInTheDocument();
    await advance(210);
    expect(
      screen.getByRole("heading", { name: "Trade page /trades/one" }),
    ).toBeInTheDocument();
    await advance(310);
    expect(screen.queryByTestId("route-transition")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByTestId("route-transition")).toBeInTheDocument();
    await advance(520);
    expect(
      screen.getByRole("heading", { name: "Pools page" }),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Forward" }));
    expect(screen.getByTestId("route-transition")).toBeInTheDocument();
    await advance(520);
    expect(
      screen.getByRole("heading", { name: "Trade page /trades/one" }),
    ).toBeInTheDocument();
  });

  it("cancels superseded timers and preserves replace and query semantics", async () => {
    renderRoutes();
    fireEvent.click(screen.getByRole("link", { name: "Trade one" }));
    await advance(100);
    fireEvent.click(screen.getByRole("button", { name: "Trade two" }));
    await advance(110);
    expect(
      screen.getByRole("heading", { name: "Pools page" }),
    ).toBeInTheDocument();
    expect(screen.getByTestId("route-transition")).toBeInTheDocument();
    await advance(100);
    expect(
      screen.getByRole("heading", {
        name: "Trade page /trades/two?source=pools",
      }),
    ).toBeInTheDocument();
    await advance(310);
    expect(screen.queryByTestId("route-transition")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    await advance(520);
    expect(
      screen.getByRole("heading", { name: "Pools page" }),
    ).toBeInTheDocument();
  });

  it("transitions automatic redirects and clears pending work on unmount", async () => {
    const view = renderRoutes("/missing");
    expect(screen.getByTestId("route-transition")).toBeInTheDocument();
    await advance(520);
    expect(
      screen.getByRole("heading", { name: "Pools page" }),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("link", { name: "Trade one" }));
    view.unmount();
    expect(vi.getTimerCount()).toBe(0);
    await advance(520);
    expect(screen.queryByTestId("route-transition")).not.toBeInTheDocument();
  });
});
