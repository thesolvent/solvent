import { useQuery } from "@tanstack/react-query";

import { useApi } from "./context";

/** Runtime config the server owns: chain, explorer, default fee, feature flags. */
export function useConfig() {
  const api = useApi();
  return useQuery({ queryKey: ["config"], queryFn: () => api.config() });
}
