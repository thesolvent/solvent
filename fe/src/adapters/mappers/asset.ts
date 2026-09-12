import type { Asset as ApiAsset } from "@solvent/sdk/client";

import { DASH, type Asset, type ChainInfo } from "@/data";

/** Signed to a leading `+`/`-`, which is how the picker decides the colour to print it in. */
function signed(pct?: number | null): string {
  return pct == null
    ? DASH
    : `${pct < 0 ? "-" : "+"}${Math.abs(pct).toFixed(2)}%`;
}

/** The chains a deployment names, as the mappers consume them. */
export function chainsOf(config: {
  chains?: { chain_id: number; name: string; logo_uri?: string | null }[];
}): ChainInfo[] {
  return (config.chains ?? []).map((chain) => ({
    chainId: chain.chain_id,
    name: chain.name,
    logoUri: chain.logo_uri,
  }));
}

/**
 * One served asset.
 *
 * The chain is resolved from the asset's own id rather than from the deployment that answered:
 * this build reads several deployments, and labelling every asset with the endpoint's own chain
 * mislabels anything sourced across one.
 */
export function toAsset(api: ApiAsset, chains: ChainInfo[]): Asset {
  const chain = chains.find((candidate) => candidate.chainId === api.chain_id);
  return {
    chainId: api.chain_id,
    address: api.address as `0x${string}`,
    symbol: api.symbol,
    name: api.name,
    logoUri: api.logo_uri,
    decimals: api.decimals,
    price: api.price_usd ?? 0,
    change: signed(api.change_24h_pct),
    tags: api.tags,
    net: chain?.name ?? "Unknown",
    chainLogoUri: chain?.logoUri,
    pairs: api.pairs,
  };
}
