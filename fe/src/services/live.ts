import { SolventApiError } from "@solvent/sdk/client";

export function isMissingRecord(error: unknown): boolean {
  return error instanceof SolventApiError && [400, 404].includes(error.status);
}

export const LIVE_QUERY_OPTIONS = {
  refetchInterval: 5_000,
  refetchOnWindowFocus: "always",
  refetchOnReconnect: "always",
} as const;
