import { describe, expect, it } from "vitest";

import fixture from "@/data/fixtures/assets.json";

import { toAsset } from "./asset";

const assets = fixture.items;

describe("toAsset", () => {
  it("signs the daily move so the picker can colour it", () => {
    // The view reads charAt(0) === "-" to decide; an unsigned number always reads as a rise.
    for (const api of assets) {
      const { change } = toAsset(api, [
        { chainId: api.chain_id, name: "Ethereum" },
      ]);
      if (api.change_24h_pct == null) continue;
      expect(change.charAt(0)).toBe(api.change_24h_pct < 0 ? "-" : "+");
    }
  });

  it("carries the address and decimals a quote needs to name the token", () => {
    const weth = assets.find((a) => a.symbol === "WETH");
    expect(
      toAsset(weth!, [{ chainId: weth!.chain_id, name: "Ethereum" }]),
    ).toMatchObject({
      address: weth!.address,
      decimals: 18,
    });
  });
});
