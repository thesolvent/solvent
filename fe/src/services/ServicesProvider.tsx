import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useState, type ReactNode } from "react";

import { ServicesCtx, type Services } from "./context";

function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: { staleTime: 10_000, retry: 1, refetchOnWindowFocus: false },
    },
  });
}

/** Wires the injected services and the server-state cache above the app. Tests pass their own
 *  `queryClient` to control retry and caching. */
export function ServicesProvider({
  services,
  queryClient,
  children,
}: {
  services: Services;
  queryClient?: QueryClient;
  children: ReactNode;
}) {
  const [fallback] = useState(createQueryClient);
  return (
    <QueryClientProvider client={queryClient ?? fallback}>
      <ServicesCtx.Provider value={services}>{children}</ServicesCtx.Provider>
    </QueryClientProvider>
  );
}
