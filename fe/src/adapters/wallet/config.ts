import { defineChain } from "viem";
import { createConfig, http } from "wagmi";
import { injected } from "wagmi/connectors";

/**
 * The chain this build talks to.
 *
 * wagmi is configured before anything is fetched, so this cannot come from `/v1/config` the way
 * the rest of the deployment's settings do — it is build-time configuration instead.
 */
const CHAIN_ID = Number(import.meta.env.VITE_CHAIN_ID ?? 31337);
const RPC_URL = import.meta.env.VITE_RPC_URL ?? "http://127.0.0.1:8545";
const CHAIN_NAME = import.meta.env.VITE_CHAIN_NAME ?? "Solvent Devnet";

export const chain = defineChain({
  id: CHAIN_ID,
  name: CHAIN_NAME,
  nativeCurrency: { name: "Ether", symbol: "ETH", decimals: 18 },
  rpcUrls: { default: { http: [RPC_URL] } },
});

/** Injected wallets only: WalletConnect needs a project id this deployment does not hold. */
export const wagmiConfig = createConfig({
  chains: [chain],
  connectors: [injected()],
  transports: { [chain.id]: http(RPC_URL) },
});
