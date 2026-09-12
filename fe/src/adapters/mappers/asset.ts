import type { Asset as ApiAsset } from "@solvent/sdk/client";

import { type Asset, type ChainInfo } from "@/data";
import { percent } from "@/lib/format";

/**
 * One served asset.
 *
 * The chain is resolved from the asset's own id rather than from the deployment that answered:
 * this build reads several deployments, and labelling every asset with the endpoint's own chain
 * mislabels anything sourced across one.
 */
export function toAsset(api: ApiAsset, chains: ChainInfo[]): Asset {
  const chain = chains.find((c) => c.chainId === api.chain_id);
  return {
    chainId: api.chain_id,
    address: api.address as `0x${string}`,
    symbol: api.symbol,
    name: api.name,
    logoUri: api.logo_uri,
    decimals: api.decimals,
    price: api.price_usd ?? null,
    // Signed to a leading `+`/`-`, which is how the picker decides the colour to print it in.
    change: percent(api.change_24h_pct, { sign: "plus" }),
    tags: api.tags,
    net: chain?.name ?? "Unknown",
    chainLogoUri: chain?.logoUri,
    pairs: api.pairs,
  };
}
