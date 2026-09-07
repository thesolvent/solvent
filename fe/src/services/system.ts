import { useQuery } from "@tanstack/react-query";

import { useServices } from "./context";

/** Runtime config the server owns: chain, explorer, default fee, feature flags. */
export function useConfig() {
  const { system } = useServices();
  return useQuery({ queryKey: ["config"], queryFn: () => system.config() });
}
