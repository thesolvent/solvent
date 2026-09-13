import { useMemo } from "react";
import { erc20Abi, formatUnits } from "viem";
import { useAccount, useReadContracts, useSwitchChain } from "wagmi";
import { usePrivy } from "@privy-io/react-auth";

import { chain } from "@/adapters/wallet/config";
import type { Asset } from "@/data";
import { formatTokenAmount } from "@/lib/format";
import { assetKey } from "@/lib/swap";

import { LIVE_QUERY_OPTIONS } from "./live";

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

/** Live, chain-qualified ERC-20 balances for the wallet currently selected in Wagmi. */
export function useAssetBalances(assets: readonly Asset[]) {
  const { address } = useAccount();
  const contracts = useMemo(
    () =>
      address
        ? assets.map((asset) => ({
            address: asset.address,
            abi: erc20Abi,
            functionName: "balanceOf" as const,
            args: [address] as const,
            chainId: asset.chainId,
          }))
        : [],
    [address, assets],
  );
  const { data } = useReadContracts({
    contracts,
    query: {
      ...LIVE_QUERY_OPTIONS,
      enabled: contracts.length > 0,
    },
  });

  return useMemo(
    () =>
      new Map(
        assets.flatMap((asset, index) => {
          const read = data?.[index];
          if (read?.status !== "success" || typeof read.result !== "bigint")
            return [];
          return [
            [assetKey(asset), displayBalance(read.result, asset.decimals)],
          ];
        }),
      ),
    [assets, data],
  );
}

/** Limits display precision without changing the wallet's exact on-chain value. */
export function displayBalance(raw: bigint, decimals: number): string {
  return formatTokenAmount(formatUnits(raw, decimals));
}
