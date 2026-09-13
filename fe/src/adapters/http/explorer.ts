import type { ExplorerPort } from "@/ports/explorer";
import { SolventApiError } from "@solvent/sdk/client";
import {
  toActivity,
  toCrossChainTrade,
  toStats,
  toTrade,
  toUniswapXFeed,
} from "../mappers/explorer";
import { toAsset } from "../mappers/asset";
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

function chain(config: {
  networks: string[];
  network_logo_uri?: string | null;
}) {
  return {
    name: config.networks[0] ?? "Unknown",
    logoUri: config.network_logo_uri,
  };
}

export const explorerAdapter: ExplorerPort = {
  async trades(filter, cursor) {
    const { chainId, ...query } = filter;
    const api = chainId == null ? solventApi : baseApi;
    const [page, config] = await Promise.all([
      api.trades({
        ...query,
        cursor,
        limit: PAGE_SIZE,
      }),
      api.config(),
    ]);
    return {
      items: page.items.map((trade) => toTrade(trade, chain(config))),
      nextCursor: page.next_cursor,
    };
  },
  async trade(id) {
    try {
      const [trade, config] = await Promise.all([
        solventApi.tradeDetail(id),
        solventApi.config(),
      ]);
      return toTrade(trade, chain(config));
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
          toAsset(
            asset,
            originConfig.networks[0] ?? "Unknown",
            originConfig.network_logo_uri,
          ),
        ),
        destinationAssets.items.map((asset) =>
          toAsset(
            asset,
            destinationConfig.networks[0] ?? "Unknown",
            destinationConfig.network_logo_uri,
          ),
        ),
      );
    }
  },
  async uniswapxFeed(cursor) {
    const [page, config] = await Promise.all([
      solventApi.uniswapxFeed({
        cursor,
        limit: PAGE_SIZE,
      }),
      solventApi.config(),
    ]);
    return {
      items: page.items.map((item) => toUniswapXFeed(item, chain(config))),
      nextCursor: page.next_cursor,
    };
  },
  async activity(filter, cursor) {
    // The Aqua feed records maker events; it does not publish resolver events.
    if (filter.entity === "Resolver")
      return { items: [], nextCursor: undefined };
    const [page, config] = await Promise.all([
      solventApi.activity({
        kind: filter.kind ? KINDS[filter.kind] : undefined,
        cursor,
        limit: PAGE_SIZE,
      }),
      solventApi.config(),
    ]);
    return {
      items: page.items.map((item) => toActivity(item, chain(config))),
      nextCursor: page.next_cursor,
    };
  },
  async stats() {
    return toStats(await solventApi.stats());
  },
};
