import { getDefaultConfig } from "@rainbow-me/rainbowkit";
import { http } from "wagmi";

import { devnet } from "./chain";
import { env } from "./env";

/** wagmi + RainbowKit config: the devnet chain, RainbowKit's default connector set (injected +
 *  WalletConnect), and an HTTP transport to the devnet RPC. */
export const wagmiConfig = getDefaultConfig({
  appName: "Solvent",
  projectId: env.walletConnectProjectId,
  chains: [devnet],
  transports: { [devnet.id]: http(env.rpcUrl) },
  ssr: false,
});
