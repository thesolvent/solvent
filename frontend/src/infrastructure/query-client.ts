import { QueryClient } from "@tanstack/react-query";

/** The server-state cache. Reads are queries; the write paths are mutations that invalidate. */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 10_000,
      retry: 1,
      refetchOnWindowFocus: false,
    },
  },
});
