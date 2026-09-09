import { makersAdapter } from "./makers";
import type { AssetsPort } from "@/ports/assets";
import type { PoolsPort } from "@/ports/pools";
import type { SystemPort } from "@/ports/system";
import type { Services } from "@/services/context";

import { toAsset } from "../mappers/asset";
import { toPool } from "../mappers/pool";
import { toDepthCurve, toPoolRoster } from "../mappers/pool-detail";
import { swapAdapter } from "./swap";
import { positionsAdapter } from "./positions";
import { solventApi } from "./client";
import { explorerAdapter } from "./explorer";
import { faucetAdapter } from "./faucet";

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
  async list() {
    const [served, config] = await Promise.all([
      solventApi.assets(),
      solventApi.config(),
    ]);
    const network = config.networks[0] ?? "Unknown";
    return served.items.map((asset) => toAsset(asset, network));
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
  swap: swapAdapter,
  system,
};
