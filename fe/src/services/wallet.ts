import { usePrivy } from "@privy-io/react-auth";
import { useAccount, useSwitchChain } from "wagmi";

import { chain } from "@/adapters/wallet/config";

/** Routes a protocol write through the active wagmi wallet on the protocol's chain. */
export function useWalletAction() {
  const { address, chainId, isConnected } = useAccount();
  const { connectOrCreateWallet } = usePrivy();
  const { switchChain } = useSwitchChain();
  const connected = isConnected || address !== undefined;
  const switchTo = connected && chainId !== chain.id ? chain.name : undefined;

  function prepare() {
    if (!connected) {
      connectOrCreateWallet();
      return false;
    }
    if (switchTo) {
      switchChain({ chainId: chain.id });
      return false;
    }
    return true;
  }

  return { address, chainId, connected, switchTo, prepare };
}
