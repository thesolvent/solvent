import { useQuery } from "@tanstack/react-query";

import type { Pool } from "@/data";

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
