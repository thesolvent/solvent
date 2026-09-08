import { useQuery } from "@tanstack/react-query";

import type { Asset } from "@/data";

import { useServices } from "./context";

/** Assets this deployment serves. Empty until the first read resolves. */
export function useAssets(): Asset[] {
  const { assets } = useServices();
  const { data } = useQuery({
    queryKey: ["assets"],
    queryFn: () => assets.list(),
  });
  return data ?? [];
}

/** Symbols alone, for the filter cells that only name assets. */
export function useAssetSymbols(): string[] {
  return useAssets().map((asset) => asset.symbol);
}
