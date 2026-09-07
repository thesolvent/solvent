import { createSolventClient, type SolventClient } from "@solvent/sdk/client";

const BASE_URL = import.meta.env.VITE_API_BASE_URL ?? "http://127.0.0.1:8080";

/** The one live API client, bound to the configured origin. */
export const solventApi: SolventClient = createSolventClient({
  baseUrl: BASE_URL,
});
