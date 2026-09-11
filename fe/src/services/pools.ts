import {
  skipToken,
  useQuery,
  type UseQueryResult,
} from "@tanstack/react-query";

import type { DepthCurve, PairRef, Pool, PoolRoster } from "@/data";

import { useServices } from "./context";
import { LIVE_QUERY_OPTIONS } from "./live";

/**
 * Every pool, as a read a caller can tell apart from an answered-and-empty one.
 *
 * The whole query is returned, not `data ?? []`: a bare array makes "not read yet" and "the API is
 * down" indistinguishable from "no pools", and the page then states the last of the three.
 */
export function usePools(): UseQueryResult<Pool[], Error> {
  const { pools } = useServices();
  return useQuery({
    ...LIVE_QUERY_OPTIONS,
    queryKey: ["pools"],
    queryFn: () => pools.list(),
  });
}

/** URL-safe form of a pair label: "WETH / USDC" -> "weth-usdc". */
export function slug(pair: string): string {
  return pair.toLowerCase().replace(/\s*\/\s*/, "-");
}

/** A read keyed by a pool's pair, idle until the list row naming that pair has loaded. */
function usePairQuery<T>(
  name: string,
  pool: Pool | undefined,
  read: (ref: PairRef) => Promise<T>,
): UseQueryResult<T, Error> {
  const ref = pool?.ref;
  return useQuery({
    ...LIVE_QUERY_OPTIONS,
    queryKey: [name, ref?.base, ref?.quote],
    queryFn: ref ? () => read(ref) : skipToken,
  });
}

export function usePoolRoster(
  pool: Pool | undefined,
): UseQueryResult<PoolRoster, Error> {
  const { pools } = useServices();
  return usePairQuery("pool-detail", pool, (ref) => pools.detail(ref));
}

export function usePoolDepth(
  pool: Pool | undefined,
): UseQueryResult<DepthCurve, Error> {
  const { pools } = useServices();
  return usePairQuery("pool-depth", pool, (ref) => pools.depth(ref));
}
