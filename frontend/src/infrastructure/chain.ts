import { defineChain } from "viem";

import { env } from "./env";

/** The Solvent devnet (anvil), described for wagmi/viem from the runtime env. */
export const devnet = defineChain({
  id: env.chainId,
  name: "Solvent Devnet",
  nativeCurrency: { name: "Ether", symbol: "ETH", decimals: 18 },
  rpcUrls: { default: { http: [env.rpcUrl] } },
});
