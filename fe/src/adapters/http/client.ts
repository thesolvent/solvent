import { createSolventClient } from "@solvent/sdk/client";

import type { SolventApi } from "@/ports/solvent-api";

const BASE_URL = import.meta.env.VITE_API_BASE_URL ?? "http://127.0.0.1:8080";

/** The one live API client, bound to the configured origin. */
export const solventApi: SolventApi = createSolventClient({
  baseUrl: BASE_URL,
});
