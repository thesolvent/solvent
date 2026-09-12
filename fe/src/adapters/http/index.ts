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
import {
  crossChainOriginApi,
  directDestinationApi,
  solventApi,
} from "./client";
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

const SOLVENTX_ORIGIN_NETWORK = "EthDevnet";
const SOLVENTX_DESTINATION_NETWORK = "BaseDevnet";

const assets: AssetsPort = {
  // A deployment names the chain it serves, so config is read alongside the assets themselves.
  async list(includeCrossChain = false) {
    const [served, config] = await Promise.all([
      solventApi.assets(),
      solventApi.config(),
    ]);
    const network = config.networks[0] ?? "Unknown";
    const primary = served.items.map((asset) => toAsset(asset, network));
    if (!includeCrossChain) return primary;

    const [originServed, destinationServed] = await Promise.all([
      crossChainOriginApi.assets(),
      directDestinationApi.assets({ supported: true }),
    ]);
    const directPair = [
      ...new Set(destinationServed.items.flatMap((asset) => asset.pairs)),
    ][0];
    const [originSymbol, destinationSymbol] = directPair?.split("/") ?? [];
    if (!originSymbol || !destinationSymbol) return [];

    const originAsset = originServed.items.find(
      (asset) => asset.symbol === originSymbol,
    );
    const destinationInput = destinationServed.items.find(
      (asset) => asset.symbol === originSymbol,
    );
    const destinationOutput = destinationServed.items.find(
      (asset) => asset.symbol === destinationSymbol,
    );
    if (!originAsset || !destinationInput || !destinationOutput) return [];

    return [
      toAsset(originAsset, SOLVENTX_ORIGIN_NETWORK),
      toAsset(destinationInput, SOLVENTX_DESTINATION_NETWORK),
      toAsset(destinationOutput, SOLVENTX_DESTINATION_NETWORK),
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
