import type { AppConfig } from "@solvent/sdk/client";

/** Server runtime settings: chain, explorer, default fee, feature flags. */
export interface SystemPort {
  config(): Promise<AppConfig>;
}
