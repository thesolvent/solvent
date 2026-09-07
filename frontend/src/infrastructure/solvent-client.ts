import { createSolventClient } from "@solvent/sdk/client";

import type { SolventClient } from "@/ports/solvent-client";

import { env } from "./env";

/** The one API client instance, bound to the configured origin. */
export const solventClient: SolventClient = createSolventClient({
  baseUrl: env.apiBaseUrl,
});
