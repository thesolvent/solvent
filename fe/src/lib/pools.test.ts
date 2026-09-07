import { describe, expect, it } from "vitest";

import type { Pool } from "@/data";
import type { PoolQuery } from "@/state";

import {
  aprOptions,
  bestByApr,
  feeTierOptions,
  filterPools,
  poolTypeOptions,
  sortPools,
} from "./pools";

const ANY: PoolQuery = {
  ptype: "All pools",
  sell: "Any",
  buy: "Any",
  fee: "Any",
  apr: "Any",
};

function pool(over: Partial<Pool> = {}): Pool {
  return {
    pair: "WETH / USDC",
    type: "Volatile",
    venue: "Aqua core · 1 maker",
    range: "0.05% spread",
    tvl: "$1.0M",
    vol: "—",
    fills: "0",
    fee: "0.05% · v1",
    feeTier: "0.05%",
    apr: "10.0%",
    tvlUsd: 1_000_000,
    aprPct: 10,
    ...over,
  };
}

const pairs = (pools: Pool[]) => pools.map((p) => p.pair);

describe("filterPools", () => {
  it("keeps everything when no cell constrains", () => {
    const pools = [pool(), pool({ pair: "DAI / USDC" })];
    expect(filterPools(pools, { query: ANY })).toHaveLength(2);
  });

  it("matches the sell and buy legs independently", () => {
    const pools = [pool(), pool({ pair: "DAI / USDT" })];

    expect(
      pairs(filterPools(pools, { query: { ...ANY, sell: "DAI" } })),
    ).toEqual(["DAI / USDT"]);
    expect(
      pairs(filterPools(pools, { query: { ...ANY, buy: "USDC" } })),
    ).toEqual(["WETH / USDC"]);
  });

  it("narrows by pool type and fee tier", () => {
    const pools = [
      pool(),
      // `fee` carries a version suffix for display, so the bare tier is what gets compared.
      pool({
        pair: "DAI / USDC",
        type: "Stable",
        fee: "0.01% · v1",
        feeTier: "0.01%",
      }),
    ];

    expect(
      pairs(filterPools(pools, { query: { ...ANY, ptype: "Stable" } })),
    ).toEqual(["DAI / USDC"]);
    expect(
      pairs(filterPools(pools, { query: { ...ANY, fee: "0.01%" } })),
    ).toEqual(["DAI / USDC"]);
  });

  it("drops an unvalued pool from an explicit threshold rather than passing it", () => {
    const unpriced = pool({ pair: "DAI / USDC", aprPct: null, tvlUsd: null });
    const pools = [pool(), unpriced];

    expect(pairs(filterPools(pools, { query: { ...ANY, apr: "5%" } }))).toEqual(
      ["WETH / USDC"],
    );
    // The slider reads in millions: 5 -> $0.5M, which the $1.0M pool clears and the unvalued one cannot.
    expect(pairs(filterPools(pools, { query: ANY, tvlSliderPct: 5 }))).toEqual([
      "WETH / USDC",
    ]);
  });

  it("narrows by the server's classification, including ones it alone derives", () => {
    // "Correlated" is a pegged-dominant pair; nothing in the pair's symbols reveals it, so a
    // client-side re-derivation could never select these pools.
    const pools = [pool(), pool({ pair: "DAI / USDC", type: "Correlated" })];

    expect(
      pairs(filterPools(pools, { query: { ...ANY, ptype: "Correlated" } })),
    ).toEqual(["DAI / USDC"]);
    expect(poolTypeOptions(pools)).toEqual(["Correlated", "Volatile"]);
  });

  it("keeps pools with at least one maker on the chosen curve", () => {
    const pools = [
      pool({ curves: ["Constant product"] }),
      pool({ pair: "DAI / USDC", curves: ["Pegged", "Concentrated"] }),
    ];

    expect(
      pairs(filterPools(pools, { query: ANY, curve: "Concentrated" })),
    ).toEqual(["DAI / USDC"]);
    // A pool mixes shapes, so a maker on any other curve must not exclude it.
    expect(filterPools(pools, { query: ANY, curve: "Any" })).toHaveLength(2);
  });

  it("applies an APR floor", () => {
    const pools = [
      pool({ aprPct: 4 }),
      pool({ pair: "DAI / USDC", aprPct: 12 }),
    ];
    expect(pairs(filterPools(pools, { query: { ...ANY, apr: "8%" } }))).toEqual(
      ["DAI / USDC"],
    );
  });
});

describe("sortPools", () => {
  const low = pool({ pair: "LOW / USDC", tvlUsd: 1, aprPct: 1 });
  const high = pool({ pair: "HIGH / USDC", tvlUsd: 9, aprPct: 9 });
  const unvalued = pool({ pair: "NONE / USDC", tvlUsd: null, aprPct: null });

  it("ranks by the requested magnitude, sinking unvalued pools", () => {
    expect(pairs(sortPools([low, unvalued, high], "Most TVL"))).toEqual([
      "HIGH / USDC",
      "LOW / USDC",
      "NONE / USDC",
    ]);
    expect(pairs(sortPools([low, unvalued, high], "Highest APR"))[0]).toBe(
      "HIGH / USDC",
    );
  });

  it("leaves the served order alone for rankings the server owns", () => {
    expect(pairs(sortPools([low, high], "Best"))).toEqual([
      "LOW / USDC",
      "HIGH / USDC",
    ]);
  });
});

describe("bestByApr", () => {
  it("picks the highest yield, not the first row", () => {
    const pools = [
      pool({ pair: "LOW / USDC", aprPct: 2 }),
      pool({ pair: "HIGH / USDC", aprPct: 18 }),
    ];
    expect(bestByApr(pools)?.pair).toBe("HIGH / USDC");
  });

  it("falls back to the leading pool while none is valued", () => {
    const pools = [
      pool({ pair: "A / USDC", aprPct: null }),
      pool({ pair: "B / USDC", aprPct: null }),
    ];
    expect(bestByApr(pools)?.pair).toBe("A / USDC");
    expect(bestByApr([])).toBeUndefined();
  });
});

describe("filter options", () => {
  it("offers only the fee tiers and yields the pools actually have, ascending", () => {
    const pools = [
      pool({ feeTier: "0.30%", aprPct: 12 }),
      pool({ pair: "DAI / USDC", feeTier: "0.01%", aprPct: 3 }),
      pool({ pair: "LINK / USDC", feeTier: "0.30%", aprPct: 12 }),
    ];

    expect(feeTierOptions(pools)).toEqual(["0.01%", "0.30%"]);
    expect(aprOptions(pools)).toEqual(["3.0%", "12.0%"]);
  });

  it("offers no yields while the server has valued none", () => {
    expect(aprOptions([pool({ aprPct: null })])).toEqual([]);
  });
});
