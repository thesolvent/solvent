import { createSolventClient, type SolventClient } from "@solvent/sdk/client";
import {
  createCrossChainClient,
  type CrossChainClient,
} from "@solvent/sdk/cross-chain";

const BASE_URL = import.meta.env.VITE_API_BASE_URL ?? "http://127.0.0.1:8080";
const BASE_CHAIN_URL =
  import.meta.env.VITE_SOLVENTX_BASE_API_URL ?? "http://127.0.0.1:8082";
const CROSS_CHAIN_ORIGIN_URL =
  import.meta.env.VITE_SOLVENTX_ORIGIN_API_URL ?? "http://127.0.0.1:8081";
const CROSS_CHAIN_PROXY_URL =
  import.meta.env.VITE_SOLVENTX_PROXY_URL ?? "/solventx-api";

/** The one live API client, bound to the configured origin. */
export const solventApi: SolventClient = createSolventClient({
  baseUrl: BASE_URL,
});

/** The second deployment SolventX composes with the primary Solvent service. */
export const baseApi: SolventClient = createSolventClient({
  baseUrl: BASE_CHAIN_URL,
});

/** The paired origin deployment used by the cross-chain coordinator. */
export const crossChainOriginApi: SolventClient = createSolventClient({
  baseUrl: CROSS_CHAIN_ORIGIN_URL,
});

/** Browser-facing coordinator for quotes spanning the paired deployments. */
export const crossChainApi: CrossChainClient = createCrossChainClient({
  baseUrl: CROSS_CHAIN_PROXY_URL,
});
