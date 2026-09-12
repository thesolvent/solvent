import type { MakersPort } from "@/ports/makers";
import {
  toDashboard,
  toInventory,
  toMaker,
  toMakerSettlement,
  toPosition,
} from "../mappers/makers";
import { toDepthCurve } from "../mappers/pool-detail";
import { baseApi, solventApi } from "./client";

function strategyApi(chainId?: number) {
  return chainId == null ? solventApi : baseApi;
}

export const makersAdapter: MakersPort = {
  async list() {
    return (await solventApi.makers()).items.map(toMaker);
  },
  async dashboard(address, period) {
    return toDashboard(await solventApi.maker(address, { period }));
  },
  async inventory(address, period) {
    return (await solventApi.makerInventory(address, { period })).items.map(
      toInventory,
    );
  },
  async positions(address, period) {
    return (await solventApi.makerPositions(address, { period })).items.map(
      toPosition,
    );
  },
  async position(hash, chainId) {
    return toPosition(await strategyApi(chainId).position(hash));
  },
  async depth(hash, pair, chainId) {
    return toDepthCurve(await strategyApi(chainId).positionDepth(hash), pair);
  },
  async history(hash, chainId) {
    const history = await strategyApi(chainId).positionHistory(hash);
    return {
      from: history.from,
      to: history.to,
      createdBlock: history.created_block ?? null,
      prices: history.prices.map((point) => ({
        at: point.at,
        price: point.price == null ? null : Number(point.price),
      })),
    };
  },
  async settlements(address, period, cursor) {
    const page = await solventApi.makerTrades(address, {
      period,
      cursor,
      limit: 10,
    });
    return {
      items: page.items.map(toMakerSettlement),
      nextCursor: page.next_cursor,
    };
  },
};
