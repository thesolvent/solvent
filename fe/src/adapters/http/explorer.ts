import type { ExplorerPort } from "@/ports/explorer";
import { SolventApiError } from "@solvent/sdk/client";
import {
  toActivity,
  toCrossChainTrade,
  toStats,
  toTrade,
  toUniswapXFeed,
} from "../mappers/explorer";
import { chainsOf, toAsset } from "../mappers/asset";
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
      // Both deployments' chains, so each asset is named by its own id, not by who answered.
      const known = [...chainsOf(originConfig), ...chainsOf(destinationConfig)];
      return toCrossChainTrade(
        order,
        originAssets.items.map((asset) => toAsset(asset, known)),
        destinationAssets.items.map((asset) => toAsset(asset, known)),
      );
    }
  },
  async uniswapxFeed(cursor) {
    const page = await solventApi.uniswapxFeed({
      cursor,
      limit: PAGE_SIZE,
    });
    return {
      items: page.items.map(toUniswapXFeed),
      nextCursor: page.next_cursor,
    };
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
