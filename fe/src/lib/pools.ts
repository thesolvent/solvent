import type { Pool } from "@/data";
import type { PoolQuery } from "@/state";

/** Cell values that express no constraint. */
const ANY = ["Any", "All pools", ""];

/** What a cell narrows by, or `undefined` when it is set to "any". */
function constraint(option?: string): string | undefined {
  return option === undefined || ANY.includes(option) ? undefined : option;
}

const legs = (pair: string): string[] => pair.split(" / ");

/** "8%" -> 8. */
function percent(option: string): number {
  return Number.parseFloat(option);
}

/** The slider reads 0–100 and is labelled in millions, so 40 means $4.0M. */
export function tvlFloorUsd(sliderPct: number): number {
  return (sliderPct / 10) * 1_000_000;
}

/** Everything the pools view narrows by: the query bar, the sidebar and the TVL slider. */
export interface PoolFilters {
  query: PoolQuery;
  /** Slider position, 0-100, labelled in millions. */
  tvlSliderPct?: number;
  /** A curve shape from the sidebar; "Any" and undefined both mean no constraint. */
  curve?: string;
}

/**
 * Narrow pools to the active filters.
 *
 * A pool the server has not valued yet fails an explicit threshold rather than passing it — an
 * unknown APR is not evidence of clearing the bar.
 */
export function filterPools(pools: Pool[], filters: PoolFilters): Pool[] {
  const { query, tvlSliderPct = 0, curve } = filters;
  const floor = tvlFloorUsd(tvlSliderPct);
  const wantType = constraint(query.ptype);
  const wantSell = constraint(query.sell);
  const wantBuy = constraint(query.buy);
  const wantFee = constraint(query.fee);
  const wantApr = constraint(query.apr);
  const wantCurve = constraint(curve);

  return pools.filter((pool) => {
    const [sell, buy] = legs(pool.pair);
    // The server classifies the pair; re-deriving it here would miss the classes it alone knows.
    if (wantType && pool.type !== wantType) return false;
    if (wantSell && sell !== wantSell) return false;
    if (wantBuy && buy !== wantBuy) return false;
    if (wantFee && (pool.feeTier ?? pool.fee) !== wantFee) return false;
    if (wantApr && (pool.aprPct ?? -Infinity) < percent(wantApr)) return false;
    if (wantCurve && !(pool.curves ?? []).includes(wantCurve)) return false;
    return floor <= 0 || (pool.tvlUsd ?? -Infinity) >= floor;
  });
}

/** The classifications present, in the server's own words. */
export function poolTypeOptions(pools: Pool[]): string[] {
  return [...new Set(pools.map((pool) => pool.type))].sort();
}

/** The fee tiers actually present, ascending. Offering a tier no pool has would filter to nothing. */
export function feeTierOptions(pools: Pool[]): string[] {
  const tiers = new Set(
    pools.map((pool) => pool.feeTier ?? pool.fee).filter(Boolean),
  );
  return [...tiers].sort((a, b) => Number.parseFloat(a) - Number.parseFloat(b));
}

/** The yields on offer, ascending, as floors to filter by. Empty until the server values them. */
export function aprOptions(pools: Pool[]): string[] {
  const rates = new Set(
    pools
      .map((pool) => pool.aprPct)
      .filter((rate): rate is number => rate != null),
  );
  return [...rates].sort((a, b) => a - b).map((rate) => `${rate.toFixed(1)}%`);
}

/** The highest-yielding pool, or the first when none is valued yet. */
export function bestByApr(pools: Pool[]): Pool | undefined {
  return pools.reduce<Pool | undefined>(
    (best, pool) =>
      (pool.aprPct ?? -Infinity) > (best?.aprPct ?? -Infinity) ? pool : best,
    pools[0],
  );
}

/** Descending by the named magnitude; unvalued pools sink rather than sorting as zero. */
function byDesc(key: "tvlUsd" | "aprPct") {
  return (a: Pool, b: Pool) => (b[key] ?? -Infinity) - (a[key] ?? -Infinity);
}

/**
 * Order the visible pools. "Best" and "Newest" keep the server's order — it already ranks, and
 * nothing served carries a creation time to sort "Newest" by.
 */
export function sortPools(pools: Pool[], sort: string): Pool[] {
  switch (sort) {
    case "Highest APR":
      return [...pools].sort(byDesc("aprPct"));
    case "Most TVL":
      return [...pools].sort(byDesc("tvlUsd"));
    default:
      return pools;
  }
}
