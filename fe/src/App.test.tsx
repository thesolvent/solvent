import { QueryClient } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { WagmiProvider, createConfig, http } from "wagmi";
import { anvil } from "wagmi/chains";

import { ServicesProvider } from "@/services/ServicesProvider";
import { INITIAL_STATE } from "@/state";
import { useAppStore } from "@/store";
import { fakeServices } from "@/test/harness";

import { App } from "./App";

vi.mock("@rainbow-me/rainbowkit", () => ({
  ConnectButton: {
    Custom: ({ children }: { children: (state: object) => React.ReactNode }) =>
      children({ mounted: true }),
  },
}));

const wagmiConfig = createConfig({
  chains: [anvil],
  transports: { [anvil.id]: http() },
});

function renderApp(path: string) {
  window.history.replaceState(null, "", path);
  return render(
    <WagmiProvider config={wagmiConfig}>
      <ServicesProvider
        services={fakeServices({})}
        queryClient={
          new QueryClient({ defaultOptions: { queries: { retry: false } } })
        }
      >
        <App />
      </ServicesProvider>
    </WagmiProvider>,
  );
}

beforeEach(() => useAppStore.setState(INITIAL_STATE));

it("gives the routed content a main landmark a skip link can reach", () => {
  renderApp("/pools");

  const main = screen.getByRole("main");
  expect(main).toHaveAttribute("id", "main");
  expect(
    screen.getByRole("link", { name: "Skip to main content" }),
  ).toHaveAttribute("href", "#main");
});

it("names the primary navigation and marks the page it is on", () => {
  renderApp("/pools");

  const nav = screen.getByRole("navigation", { name: "Primary" });
  expect(nav).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Pools" })).toHaveAttribute(
    "aria-current",
    "page",
  );
  expect(screen.getByRole("button", { name: "Swap" })).not.toHaveAttribute(
    "aria-current",
  );
});
