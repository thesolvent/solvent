import { skipToken, useInfiniteQuery, useQuery } from "@tanstack/react-query";
import type { ActivityFilter, TradeFilter } from "@/ports/explorer";
import { SolventApiError } from "@solvent/sdk/client";
import { CrossChainApiError } from "@solvent/sdk/cross-chain";
import { isTerminalTrade } from "@/lib/trade-lifecycle";
import { useServices } from "./context";
import { LIVE_QUERY_OPTIONS } from "./live";

export function useTrades(filter: TradeFilter | undefined, cursor?: string) {
  const { explorer } = useServices();
  return useQuery({
    ...LIVE_QUERY_OPTIONS,
    queryKey: ["trades", filter, cursor],
    queryFn: filter ? () => explorer.trades(filter, cursor) : skipToken,
  });
}

export function useActivity(filter: ActivityFilter) {
  const { explorer } = useServices();
  return useInfiniteQuery({
    ...LIVE_QUERY_OPTIONS,
    queryKey: ["activity", filter],
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) => explorer.activity(filter, pageParam),
    getNextPageParam: (page) => page.nextCursor,
  });
}

export function useExplorerStats() {
  const { explorer } = useServices();
  return useQuery({
    ...LIVE_QUERY_OPTIONS,
    queryKey: ["stats"],
    queryFn: () => explorer.stats(),
  });
}

export function useTrade(id: string | undefined) {
  const { explorer } = useServices();
  return useQuery({
    queryKey: ["trade", id],
    queryFn: id ? () => explorer.trade(id) : skipToken,
    staleTime: 0,
    refetchOnWindowFocus: (query) => !isMissingTrade(query.state.error),
    refetchOnReconnect: (query) => !isMissingTrade(query.state.error),
    retry: (failureCount, error) => !isMissingTrade(error) && failureCount < 1,
    refetchInterval: (query) =>
      isMissingTrade(query.state.error) ||
      (query.state.data && isTerminalTrade(query.state.data.status))
        ? false
        : 2_000,
  });
}

function isMissingTrade(error: unknown): boolean {
  return (
    (error instanceof SolventApiError || error instanceof CrossChainApiError) &&
    [400, 404].includes(error.status)
  );
}

export function tradeProblem(error: unknown): string {
  if (isMissingTrade(error)) return "Trade not found.";
  return "Couldn’t load this trade. Try again.";
}

/** Every order the feed showed us — the denominator behind the trade list. */
export function useObservedOrders() {
  const { explorer } = useServices();
  return useQuery({
    ...LIVE_QUERY_OPTIONS,
    queryKey: ["observed-orders"],
    queryFn: () => explorer.orders(200),
  });
}
