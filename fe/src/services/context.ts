import type { MakersPort } from "@/ports/makers";
import { createContext, useContext } from "react";

import type { AssetsPort } from "@/ports/assets";
import type { ExplorerPort } from "@/ports/explorer";
import type { PoolsPort } from "@/ports/pools";
import type { PositionsPort } from "@/ports/positions";
import type { SwapPort } from "@/ports/swap";
import type { SystemPort } from "@/ports/system";

/** Outbound dependencies, injected at the composition root so services never import adapters. */
export interface Services {
  makers: MakersPort;
  explorer: ExplorerPort;
  assets: AssetsPort;
  pools: PoolsPort;
  positions: PositionsPort;
  swap: SwapPort;
  system: SystemPort;
}

export const ServicesCtx = createContext<Services | null>(null);

export function useServices(): Services {
  const services = useContext(ServicesCtx);
  if (!services)
    throw new Error("useServices must be used inside <ServicesProvider>");
  return services;
}
