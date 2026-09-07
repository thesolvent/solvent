import { useQuery } from "@tanstack/react-query";

import { useSolventClient } from "./services";

/** Runtime config from the API (chain, explorer, fee, feature flags). */
export function useConfig() {
  const client = useSolventClient();
  return useQuery({ queryKey: ["config"], queryFn: () => client.config() });
}

/** Global protocol stats (block height, active makers, settled trades…). */
export function useStats() {
  const client = useSolventClient();
  return useQuery({ queryKey: ["stats"], queryFn: () => client.stats() });
}
