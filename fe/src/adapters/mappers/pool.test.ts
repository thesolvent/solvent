import type { Pool as ApiPool } from "@solvent/sdk/client";
import { describe, expect, it } from "vitest";

import { poolType } from "@/lib/format";

import fixture from "@/data/fixtures/pools.json";
import { toPool } from "./pool";

/** Captured from the devnet server, so a served-shape change breaks these rather than a view. */
const pools = fixture.items as ApiPool[];

describe("toPool", () => {
  it("formats a priced pool into the record the views render", () => {
    const weth = pools.find((p) => p.base.symbol === "WETH");
    expect(weth).toBeDefined();
    const mapped = toPool(weth!);

    expect(mapped.pair).toBe("WETH / USDC");
    expect(mapped.venue).toBe("Aqua core · 1 maker");
    expect(mapped.range).toBe("0.05% spread");
    expect(mapped.tvl).toMatch(/^\$\d/);
    expect(mapped.fee).toBe("0.05% · v1");
  });

  it("names only the curve shapes a pool's makers actually price on", () => {
    const pegged = pools.find((p) => p.curve_mix.pegged > 0);
    expect(pegged).toBeDefined();

    expect(toPool(pegged!).curves).toEqual(["Pegged"]);
  });

  it("renders absent server values as text, never NaN or undefined", () => {
    const mapped = toPool({
      ...pools[0],
      tvl_usd: null,
      volume_24h_usd: null,
      apr_pct: null,
    });

    expect(mapped.tvl).toBe("—");
    expect(mapped.vol).toBe("—");
    expect(mapped.apr).toBe("—");
  });

  it("emits a pair the pool-type filter can classify", () => {
    const stable = pools.find((p) => p.type === "Stable");
    expect(stable).toBeDefined();

    // poolType() splits on " / "; an unspaced pair would silently classify everything as Volatile.
    expect(poolType(toPool(stable!))).toBe("Stable");
  });
});
