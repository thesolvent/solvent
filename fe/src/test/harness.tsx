import { QueryClient } from "@tanstack/react-query";
import { render, type RenderResult } from "@testing-library/react";
import type { ReactElement } from "react";

import { AppProvider } from "@/AppProvider";
import type { AssetsPort } from "@/ports/assets";
import type { PoolsPort } from "@/ports/pools";
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

export interface Stubs {
  assets?: Partial<AssetsPort>;
  pools?: Partial<PoolsPort>;
  system?: Partial<SystemPort>;
}

export function fakeServices(stubs: Stubs): Services {
  return {
    assets: port("assets", stubs.assets ?? {}),
    pools: port("pools", stubs.pools ?? {}),
    system: port("system", stubs.system ?? {}),
  };
}

/** Render `ui` against stubbed ports — no network, and failures surface at once (no retries). */
export function renderWithServices(
  ui: ReactElement,
  stubs: Stubs = {},
): RenderResult {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: 0 } },
  });
  return render(
    <ServicesProvider services={fakeServices(stubs)} queryClient={queryClient}>
      <AppProvider>{ui}</AppProvider>
    </ServicesProvider>,
  );
}
