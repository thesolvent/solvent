import { describe, expect, it } from "vitest";

import fixture from "@/data/fixtures/assets.json";

import { chainsOf, toAsset } from "./asset";

const assets = fixture.items;
const CHAINS = chainsOf({
  chains: [
    { chain_id: 31337, name: "Ethereum", logo_uri: "/chains/ethereum.png" },
  ],
});

describe("toAsset", () => {
  it("signs the daily move so the picker can colour it", () => {
    // The view reads charAt(0) === "-" to decide; an unsigned number always reads as a rise.
    for (const api of assets) {
      const { change } = toAsset(api, CHAINS);
      if (api.change_24h_pct == null) continue;
      expect(change.charAt(0)).toBe(api.change_24h_pct < 0 ? "-" : "+");
    }
  });

  it("carries the address and decimals a quote needs to name the token", () => {
    const weth = assets.find((a) => a.symbol === "WETH");
    expect(toAsset(weth!, CHAINS)).toMatchObject({
      address: weth!.address,
      decimals: 18,
    });
  });

  it("names an asset from its own chain id, not from the first chain offered", () => {
    const weth = assets.find((a) => a.symbol === "WETH");
    const elsewhere = [
      { chainId: 8453, name: "Base", logoUri: null },
      ...CHAINS,
    ];
    expect(toAsset(weth!, elsewhere)).toMatchObject({
      net: "Ethereum",
      chainLogoUri: "/chains/ethereum.png",
    });
  });

  it("falls back to Unknown when no chain claims the asset's id", () => {
    const weth = assets.find((a) => a.symbol === "WETH");
    expect(toAsset(weth!, []).net).toBe("Unknown");
  });
});
