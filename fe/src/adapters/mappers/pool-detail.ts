import type {
  PoolDepth as ApiDepth,
  PoolDetail as ApiDetail,
} from "@solvent/sdk/client";

import type {
  DepthCurve,
  DepthLevel,
  PairRef,
  PoolRoster,
  RosterMaker,
} from "@/data";

import { toPool } from "./pool";

/** Base-unit integer string to whole tokens. The server sends amounts unscaled. */
function whole(baseUnits: string, decimals: number): number {
  return Number(baseUnits) / 10 ** decimals;
}

type ApiMaker = ApiDetail["makers"][number];

function toRosterMaker(api: ApiMaker): RosterMaker {
  return {
    address: api.maker,
    strategyHash: api.strategy_hash,
    curve: api.curve,
    feeBps: api.fee_bps,
    virtualUsd: api.virtual.total_usd ?? null,
    actualUsd: api.actual?.total_usd ?? null,
    balances: api.virtual.entries.map((entry) => ({
      symbol: entry.token.symbol,
      display: entry.amount.display,
      usd: entry.amount.usd ?? null,
    })),
  };
}

/** The pool row plus its maker roster, ordered by committed value so the largest reads first. */
export function toPoolRoster(api: ApiDetail): PoolRoster {
  return {
    pool: toPool(api),
    makers: api.makers
      .map(toRosterMaker)
      .sort(
        (a, b) => (b.virtualUsd ?? -Infinity) - (a.virtualUsd ?? -Infinity),
      ),
  };
}

/**
 * The depth curve in whole tokens.
 *
 * Sizes are quoted in the pair's own units — input in the base token, output in the quote — so
 * scaling needs both decimals rather than one.
 */
export function toDepthCurve(api: ApiDepth, pair: PairRef): DepthCurve {
  const best = Number.parseFloat(api.best_price);
  const levels: DepthLevel[] = api.points.map((point) => ({
    sizeIn: whole(point.trade_size, pair.baseDecimals),
    output: whole(point.output, pair.quoteDecimals),
    price: Number.parseFloat(point.effective_price),
    impactPct: point.impact_pct,
    makersUsed: point.makers_used,
  }));

  return {
    axisTitle: api.axis_title,
    bestPrice: Number.isFinite(best) ? best : null,
    levels,
  };
}
