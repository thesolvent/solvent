import type { MakersPort } from "@/ports/makers";
import { QueryClient } from "@tanstack/react-query";
import { render, type RenderResult } from "@testing-library/react";
import type { ReactElement } from "react";
import { BrowserRouter, MemoryRouter } from "react-router-dom";
import { WagmiProvider, createConfig, http } from "wagmi";
import { mock } from "wagmi/connectors";
import { anvil } from "wagmi/chains";

import { AppProvider } from "@/AppProvider";
import type { AssetsPort } from "@/ports/assets";
import type { ExplorerPort } from "@/ports/explorer";
import type { PoolsPort } from "@/ports/pools";
import type { PositionsPort } from "@/ports/positions";
import type { SwapPort } from "@/ports/swap";
import type { SystemPort } from "@/ports/system";
import { ServicesProvider } from "@/services/ServicesProvider";
import type { Services } from "@/services/context";

/** A port implementing only the calls a test stubs; anything else is a mistake worth failing on
 *  rather than silently returning undefined. */
function port<T extends object>(name: string, stubs: Partial<T>): T {
  return new Proxy({} as T, {
    get(_target, key: string) {
      const stub = (stubs as Record<string, unknown>)[key];
      if (stub) return stub;
      throw new Error(`${name}.${key}() was called but not stubbed`);
    },
  });
}

/** A wallet that is present but not connected, which is what a view sees before anyone connects. */
const wagmiConfig = createConfig({
  chains: [anvil],
  connectors: [
    mock({ accounts: ["0x0000000000000000000000000000000000000001"] }),
  ],
  transports: { [anvil.id]: http() },
});

export interface Stubs {
  makers?: Partial<MakersPort>;
  explorer?: Partial<ExplorerPort>;
  assets?: Partial<AssetsPort>;
  pools?: Partial<PoolsPort>;
  positions?: Partial<PositionsPort>;
  swap?: Partial<SwapPort>;
  system?: Partial<SystemPort>;
}

export function fakeServices(stubs: Stubs): Services {
  return {
    makers: port("makers", stubs.makers ?? {}),
    explorer: port("explorer", stubs.explorer ?? {}),
    assets: port("assets", stubs.assets ?? {}),
    pools: port("pools", stubs.pools ?? {}),
    positions: port("positions", stubs.positions ?? {}),
    swap: port("swap", stubs.swap ?? {}),
    system: port("system", stubs.system ?? {}),
  };
}

/** Render `ui` against stubbed ports — no network, and failures surface at once (no retries). */
export function renderWithServices(
  ui: ReactElement,
  stubs: Stubs = {},
  route = "/",
  router: "memory" | "browser" = "memory",
): RenderResult {
  if (router === "browser") window.history.replaceState(null, "", route);
  const app = <AppProvider>{ui}</AppProvider>;
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: 0 } },
  });
  return render(
    <WagmiProvider config={wagmiConfig}>
      <ServicesProvider
        services={fakeServices(stubs)}
        queryClient={queryClient}
      >
        {router === "browser" ? (
          <BrowserRouter>{app}</BrowserRouter>
        ) : (
          <MemoryRouter initialEntries={[route]}>{app}</MemoryRouter>
        )}
      </ServicesProvider>
    </WagmiProvider>,
  );
}
