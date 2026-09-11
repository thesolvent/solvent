import { useQuery, type UseQueryResult } from "@tanstack/react-query";

import type { Asset } from "@/data";

import { useServices } from "./context";
import { LIVE_QUERY_OPTIONS } from "./live";

/**
 * Assets this deployment serves, as a read a caller can tell apart from an answered-and-empty one.
 *
 * Returning `data ?? []` collapsed first fetch and outage into "no assets", which the token picker
 * then reported as a filter that matched nothing.
 */
export function useAssets(
  includeCrossChain = false,
): UseQueryResult<Asset[], Error> {
  const { assets } = useServices();
  return useQuery({
    ...LIVE_QUERY_OPTIONS,
    queryKey: ["assets", includeCrossChain],
    queryFn: () => assets.list(includeCrossChain),
  });
}

/** Symbols alone, for the filter cells that only name assets. */
export function useAssetSymbols(): string[] {
  return (useAssets().data ?? []).map((asset) => asset.symbol);
}
