import { useMemo } from "react";

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

/**
 * Look an icon up by symbol.
 *
 * Most views render a pair label ("DAI/USDC") rather than the token objects behind it, so the
 * catalog is the one place every surface can reach an icon from.
 */
export function useTokenIcon(): (symbol: string) => string | null {
  const assets = useAssets();
  return useMemo(() => {
    const bySymbol = new Map(
      assets.map((asset) => [asset.symbol.toUpperCase(), asset.logoUri]),
    );
    return (symbol: string) => bySymbol.get(symbol.toUpperCase()) ?? null;
  }, [assets]);
}
