import type { AssetsPort } from "@/ports/assets";
import type { PoolsPort } from "@/ports/pools";
import type { SystemPort } from "@/ports/system";
import type { Services } from "@/services/context";

import { toPool } from "../mappers/pool";
import { solventApi } from "./client";

const pools: PoolsPort = {
  async list() {
    return (await solventApi.pools()).items.map(toPool);
  },
};

const assets: AssetsPort = {
  async symbols() {
    return (await solventApi.assets()).items.map((asset) => asset.symbol);
  },
};

const system: SystemPort = {
  config: () => solventApi.config(),
};

/** The live implementations the composition root injects. */
export const httpServices: Services = { assets, pools, system };
