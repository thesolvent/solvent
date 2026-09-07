import { createContext, useContext } from "react";

import type { SolventApi } from "@/ports/solvent-api";

/** Outbound dependencies, injected at the composition root so services never import adapters. */
export interface Services {
  api: SolventApi;
}

export const ServicesCtx = createContext<Services | null>(null);

export function useServices(): Services {
  const services = useContext(ServicesCtx);
  if (!services)
    throw new Error("useServices must be used inside <ServicesProvider>");
  return services;
}

export const useApi = (): SolventApi => useServices().api;
