import { skipToken, useQuery } from "@tanstack/react-query";

import type { DepthCurve, PairRef, Pool, PoolRoster } from "@/data";

import { useServices } from "./context";

/** Every pool, already formatted for display. Empty until the first read resolves. */
export function usePools(): Pool[] {
  const { pools } = useServices();
  const { data } = useQuery({
    queryKey: ["pools"],
    queryFn: () => pools.list(),
  });
  return data ?? [];
}

/** The pool a URL names, once the list it belongs to has loaded. */
export function usePool(pair: string | undefined): Pool | undefined {
  return usePools().find((pool) => slug(pool.pair) === pair);
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
): T | undefined {
  const ref = pool?.ref;
  const { data } = useQuery({
    queryKey: [name, ref?.base, ref?.quote],
    queryFn: ref ? () => read(ref) : skipToken,
  });
  return data;
}

export function usePoolRoster(pool: Pool | undefined): PoolRoster | undefined {
  const { pools } = useServices();
  return usePairQuery("pool-detail", pool, (ref) => pools.detail(ref));
}

export function usePoolDepth(pool: Pool | undefined): DepthCurve | undefined {
  const { pools } = useServices();
  return usePairQuery("pool-depth", pool, (ref) => pools.depth(ref));
}
