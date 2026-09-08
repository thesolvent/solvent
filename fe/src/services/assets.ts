import { useQuery } from "@tanstack/react-query";

import type { Asset } from "@/data";

import { useServices } from "./context";
import { LIVE_QUERY_OPTIONS } from "./live";

/** Assets this deployment serves. Empty until the first read resolves. */
export function useAssets(): Asset[] {
  const { assets } = useServices();
  const { data } = useQuery({
    ...LIVE_QUERY_OPTIONS,
    queryKey: ["assets"],
    queryFn: () => assets.list(),
  });
  return data ?? [];
}

/** Symbols alone, for the filter cells that only name assets. */
export function useAssetSymbols(): string[] {
  return useAssets().map((asset) => asset.symbol);
}
