import { useQuery } from "@tanstack/react-query";

import { useServices } from "./context";

/** Symbols of the assets this deployment supports. Empty until the first read resolves. */
export function useAssetSymbols(): string[] {
  const { assets } = useServices();
  const { data } = useQuery({
    queryKey: ["assets", "symbols"],
    queryFn: () => assets.symbols(),
  });
  return data ?? [];
}
