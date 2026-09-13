import { defineChain } from "viem";
import { createConfig } from "@privy-io/wagmi";
import { http } from "wagmi";

/**
 * The chain this build talks to.
 *
 * wagmi is configured before anything is fetched, so this cannot come from `/v1/config` the way
 * the rest of the deployment's settings do — it is build-time configuration instead.
 */
const CHAIN_ID = Number(import.meta.env.VITE_CHAIN_ID ?? 31337);
const RPC_URL = import.meta.env.VITE_RPC_URL ?? "http://127.0.0.1:9645";
const CHAIN_NAME = import.meta.env.VITE_CHAIN_NAME ?? "EthDevnet";
const DESTINATION_CHAIN_ID = Number(
  import.meta.env.VITE_DESTINATION_CHAIN_ID ?? 31338,
);
const DESTINATION_RPC_URL =
  import.meta.env.VITE_DESTINATION_RPC_URL ?? "http://127.0.0.1:9646";
const DESTINATION_CHAIN_NAME =
  import.meta.env.VITE_DESTINATION_CHAIN_NAME ?? "BaseDevnet";

export const chain = defineChain({
  id: CHAIN_ID,
  name: CHAIN_NAME,
  nativeCurrency: { name: "Ether", symbol: "ETH", decimals: 18 },
  rpcUrls: { default: { http: [RPC_URL] } },
});

export const destinationChain = defineChain({
  id: DESTINATION_CHAIN_ID,
  name: DESTINATION_CHAIN_NAME,
  nativeCurrency: { name: "Ether", symbol: "ETH", decimals: 18 },
  rpcUrls: { default: { http: [DESTINATION_RPC_URL] } },
});

export const wagmiConfig = createConfig({
  chains: [chain, destinationChain],
  transports: {
    [chain.id]: http(RPC_URL),
    [destinationChain.id]: http(DESTINATION_RPC_URL),
  },
});
