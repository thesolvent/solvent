import type { ExplorerPort } from "@/ports/explorer";
import { SolventApiError } from "@solvent/sdk/client";
import {
  toActivity,
  toCrossChainTrade,
  toStats,
  toTrade,
} from "../mappers/explorer";
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

import {
  baseApi,
  crossChainApi,
  crossChainOriginApi,
  solventApi,
} from "./client";

const KINDS: Record<string, string> = {
  register: "shipped",
  push: "pushed",
  pull: "pulled",
  dock: "docked",
};
const PAGE_SIZE = 10;

export const explorerAdapter: ExplorerPort = {
  async trades(filter, cursor) {
    const { chainId, ...query } = filter;
    const page = await (chainId == null ? solventApi : baseApi).trades({
      ...query,
      cursor,
      limit: PAGE_SIZE,
    });
    return { items: page.items.map(toTrade), nextCursor: page.next_cursor };
  },
  async trade(id) {
    try {
      return toTrade(await solventApi.tradeDetail(id));
    } catch (error) {
      if (
        !(error instanceof SolventApiError) ||
        ![400, 404].includes(error.status)
      )
        throw error;
      const [
        order,
        originAssets,
        originConfig,
        destinationAssets,
        destinationConfig,
      ] = await Promise.all([
        crossChainApi.order(id),
        crossChainOriginApi.assets(),
        crossChainOriginApi.config(),
        baseApi.assets(),
        baseApi.config(),
      ]);
      return toCrossChainTrade(
        order,
        originAssets.items.map((asset) =>
          toAsset(asset, [
            ...chainsOf(originConfig),
            ...chainsOf(destinationConfig),
          ]),
        ),
        destinationAssets.items.map((asset) =>
          toAsset(asset, [
            ...chainsOf(originConfig),
            ...chainsOf(destinationConfig),
          ]),
        ),
      );
    }
  },
  async crossChainOrders(taker) {
    const [
      orders,
      originAssets,
      originConfig,
      destinationAssets,
      destinationConfig,
    ] = await Promise.all([
      crossChainApi.ordersOf(taker),
      crossChainOriginApi.assets(),
      crossChainOriginApi.config(),
      baseApi.assets(),
      baseApi.config(),
    ]);
    // Either deployment can name either chain, so both leg mappings see the whole set.
    const known = [...chainsOf(originConfig), ...chainsOf(destinationConfig)];
    const origin = originAssets.items.map((asset) => toAsset(asset, known));
    const destination = destinationAssets.items.map((asset) =>
      toAsset(asset, known),
    );
    return orders.map((order) => toCrossChainTrade(order, origin, destination));
  },
  async activity(filter, cursor) {
    // The Aqua feed records maker events; it does not publish resolver events.
    if (filter.entity === "Resolver")
      return { items: [], nextCursor: undefined };
    const page = await solventApi.activity({
      kind: filter.kind ? KINDS[filter.kind] : undefined,
      cursor,
      limit: PAGE_SIZE,
    });
    return { items: page.items.map(toActivity), nextCursor: page.next_cursor };
  },
  async stats() {
    return toStats(await solventApi.stats());
  },
};
