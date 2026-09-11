import { erc20Abi, type Address } from "viem";
import { useAccount, useReadContract } from "wagmi";

import { chain } from "@/adapters/wallet/config";
import type { Asset } from "@/data";

/**
 * What the connected wallet holds of one asset, in base units.
 *
 * `undefined` covers every case where the holding is not known — no wallet, another chain, a read
 * still open or failed — so a caller can only ever act on a balance it actually has.
 */
export function useTokenBalance(asset: Asset | undefined): bigint | undefined {
  const { address } = useAccount();
  const readable =
    address !== undefined && asset !== undefined && asset.chainId === chain.id;
  const { data } = useReadContract({
    address: asset?.address,
    abi: erc20Abi,
    functionName: "balanceOf",
    args: [address ?? ("0x" as Address)],
    chainId: chain.id,
    query: { enabled: readable },
  });
  return readable ? data : undefined;
}
