import type { ExplorerPort } from "@/ports/explorer";
import {
  toActivity,
  toObservedOrder,
  toStats,
  toTrade,
} from "../mappers/explorer";
import { solventApi } from "./client";

const KINDS: Record<string, string> = {
  register: "shipped",
  push: "pushed",
  pull: "pulled",
  dock: "docked",
};
const PAGE_SIZE = 10;

export const explorerAdapter: ExplorerPort = {
  async trades(filter, cursor) {
    const page = await solventApi.trades({
      ...filter,
      cursor,
      limit: PAGE_SIZE,
    });
    return { items: page.items.map(toTrade), nextCursor: page.next_cursor };
  },
  async orders({ tokenIn, tokenOut, ...query } = {}) {
    const page = await solventApi.orders({
      ...query,
      token_in: tokenIn,
      token_out: tokenOut,
    });
    return { items: page.items.map(toObservedOrder), total: page.total };
  },
  async trade(id) {
    return toTrade(await solventApi.tradeDetail(id));
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
