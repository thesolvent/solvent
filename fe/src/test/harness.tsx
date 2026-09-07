import { QueryClient } from "@tanstack/react-query";
import { render, type RenderResult } from "@testing-library/react";
import type { ReactElement } from "react";

import type { SolventApi } from "@/ports/solvent-api";
import { ServicesProvider } from "@/services/ServicesProvider";

/** Build a stand-in API implementing only the calls a test exercises; anything else is a mistake
 *  worth failing on rather than silently returning undefined. */
export function fakeApi(methods: Partial<SolventApi>): SolventApi {
  return new Proxy(methods as SolventApi, {
    get(target, key: string) {
      const method = target[key as keyof SolventApi];
      if (method) return method;
      throw new Error(`fakeApi: ${key}() was called but not stubbed`);
    },
  });
}

/** Render `ui` against a fake API — no network, and failures surface immediately (no retries). */
export function renderWithServices(
  ui: ReactElement,
  api: Partial<SolventApi>,
): RenderResult {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: 0 } },
  });
  return render(
    <ServicesProvider
      services={{ api: fakeApi(api) }}
      queryClient={queryClient}
    >
      {ui}
    </ServicesProvider>,
  );
}
