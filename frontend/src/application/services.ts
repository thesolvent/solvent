import { createContext, useContext } from "react";

import type { Faucet } from "@/ports/faucet";
import type { SolventClient } from "@/ports/solvent-client";

/** The injected outbound dependencies. Concrete instances are wired in `app/`; consumers read
 *  them through these hooks, so no view or application code imports infrastructure directly. */
export interface Services {
  client: SolventClient;
  faucet: Faucet;
}

export const ServicesContext = createContext<Services | null>(null);

export function useServices(): Services {
  const services = useContext(ServicesContext);
  if (!services) throw new Error("ServicesContext is missing from the tree");
  return services;
}

export const useSolventClient = (): SolventClient => useServices().client;
export const useFaucetClient = (): Faucet => useServices().faucet;
