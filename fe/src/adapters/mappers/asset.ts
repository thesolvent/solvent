import type { Asset as ApiAsset } from "@solvent/sdk/client";

import { DASH, type Asset } from "@/data";

/** Signed to a leading `+`/`-`, which is how the picker decides the colour to print it in. */
function signed(pct?: number | null): string {
  return pct == null
    ? DASH
    : `${pct < 0 ? "-" : "+"}${Math.abs(pct).toFixed(2)}%`;
}

/**
 * One served asset.
 *
 * The network name comes from config rather than from the asset: a deployment names the chain it
 * serves, while the asset only reports the id.
 */
export function toAsset(api: ApiAsset, network: string): Asset {
  return {
    address: api.address,
    symbol: api.symbol,
    name: api.name,
    decimals: api.decimals,
    price: api.price_usd ?? 0,
    change: signed(api.change_24h_pct),
    tags: api.tags,
    net: network,
    pairs: api.pairs,
  };
}
