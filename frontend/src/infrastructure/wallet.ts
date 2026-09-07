import { useAccount } from "wagmi";

import type { Wallet } from "@/ports/wallet";

/** Adapts wagmi's account state to the app's `Wallet` port. */
export function useWagmiWallet(): Wallet {
  const { address, chainId, isConnected } = useAccount();
  return { address, chainId, isConnected };
}
