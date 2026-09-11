import { makersAdapter } from "./makers";
import type { AssetsPort } from "@/ports/assets";
import type { PoolsPort } from "@/ports/pools";
import type { SystemPort } from "@/ports/system";
import type { Services } from "@/services/context";

import { toAsset } from "../mappers/asset";

/** The chains a deployment names, as the mappers consume them. */
function chainsOf(config: {
  chains?: { chain_id: number; name: string; logo_uri?: string | null }[];
}) {
  return (config.chains ?? []).map((c) => ({
    chainId: c.chain_id,
    name: c.name,
    logoUri: c.logo_uri,
  }));
}

import { toPool } from "../mappers/pool";
import { toDepthCurve, toPoolRoster } from "../mappers/pool-detail";
import { swapAdapter } from "./swap";
import { positionsAdapter } from "./positions";
import { baseApi, solventApi } from "./client";
import { explorerAdapter } from "./explorer";
import { faucetAdapter } from "./faucet";
import { rebatesAdapter } from "./rebates";

const pools: PoolsPort = {
  async list() {
    return (await solventApi.pools()).items.map(toPool);
  },
  async detail(pair) {
    return toPoolRoster(
      await solventApi.poolDetail({ base: pair.base, quote: pair.quote }),
    );
  },
  async depth(pair) {
    return toDepthCurve(
      await solventApi.poolDepth({ base: pair.base, quote: pair.quote }),
      pair,
    );
  },
};

const assets: AssetsPort = {
  // A deployment names the chain it serves, so config is read alongside the assets themselves.
  async list(includeCrossChain = false) {
    const [served, config] = await Promise.all([
      solventApi.assets(),
      solventApi.config(),
    ]);
    const primary = served.items.map((asset) =>
      toAsset(asset, chainsOf(config)),
    );
    if (!includeCrossChain) return primary;

    const [baseServed, baseConfig] = await Promise.all([
      baseApi.assets(),
      baseApi.config(),
    ]);
    // Both deployments' chains, so an asset is named by its own id whichever answered for it.
    const known = [...chainsOf(config), ...chainsOf(baseConfig)];
    return [
      ...served.items.map((asset) => toAsset(asset, known)),
      ...baseServed.items.map((asset) => toAsset(asset, known)),
    ];
  },
};

const system: SystemPort = {
  config: () => solventApi.config(),
};

/** The live implementations the composition root injects. */
export const httpServices: Services = {
  makers: makersAdapter,
  explorer: explorerAdapter,
  faucet: faucetAdapter,
  assets,
  pools,
  positions: positionsAdapter,
  rebates: rebatesAdapter,
  swap: swapAdapter,
  system,
};
