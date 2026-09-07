import { ABI, AquaProtocolContract, HexString } from "@1inch/aqua-sdk";
import { decodeFunctionData, erc20Abi } from "viem";
import { describe, expect, it } from "vitest";

import { Strategy } from "../../src/construction/index";
import { positions } from "../../src/positions/index";
import type { Address } from "../../src/index";

const AQUA = "0x9999999999999999999999999999999999999999" as Address;
const APP = "0x8888888888888888888888888888888888888888" as Address;
const MAKER = "0x1111111111111111111111111111111111111111" as Address;
const WETH = "0x2222222222222222222222222222222222222222" as Address;
const USDC = "0x3333333333333333333333333333333333333333" as Address;

const lower = (s: unknown) => String(s).toLowerCase();
const pos = positions({ aqua: AQUA, app: APP });
const built = Strategy.fullRange().build(MAKER);

describe("positions.approve", () => {
  it("encodes ERC-20 approve to the token, spender = aqua", () => {
    const tx = pos.approve({ token: WETH, amount: 1000n });
    expect(lower(tx.to)).toBe(lower(WETH));
    expect(tx.value).toBe(0n);
    const { functionName, args } = decodeFunctionData({ abi: erc20Abi, data: tx.data });
    const a = args as readonly unknown[];
    expect(functionName).toBe("approve");
    expect(lower(a[0])).toBe(lower(AQUA));
    expect(a[1]).toBe(1000n);
  });
});

describe("positions.ship", () => {
  it("encodes ship to aqua with app / strategy / tokens / amounts", () => {
    const tx = pos.ship({
      strategy: built.order,
      amounts: [
        { token: WETH, amount: 10n },
        { token: USDC, amount: 20n },
      ],
    });
    expect(lower(tx.to)).toBe(lower(AQUA));
    expect(tx.value).toBe(0n);
    const { functionName, args } = decodeFunctionData({ abi: ABI.AQUA_ABI, data: tx.data });
    const a = args as readonly unknown[];
    expect(functionName).toBe("ship");
    expect(lower(a[0])).toBe(lower(APP));
    expect(a[1]).toBe(built.order);
    expect((a[2] as string[]).map(lower)).toEqual([WETH, USDC].map(lower));
    expect(a[3]).toEqual([10n, 20n]);
  });
});

describe("positions.dock", () => {
  it("encodes dock to aqua with app / strategyHash / tokens", () => {
    const tx = pos.dock({ strategyHash: built.strategyHash, tokens: [WETH, USDC] });
    expect(lower(tx.to)).toBe(lower(AQUA));
    expect(tx.value).toBe(0n);
    const { functionName, args } = decodeFunctionData({ abi: ABI.AQUA_ABI, data: tx.data });
    const a = args as readonly unknown[];
    expect(functionName).toBe("dock");
    expect(lower(a[0])).toBe(lower(APP));
    expect(lower(a[1])).toBe(lower(built.strategyHash));
    expect((a[2] as string[]).map(lower)).toEqual([WETH, USDC].map(lower));
  });
});

describe("positions.push", () => {
  it("encodes push to aqua with maker / app / strategyHash / token / amount", () => {
    const tx = pos.push({ maker: MAKER, strategyHash: built.strategyHash, token: WETH, amount: 5n });
    expect(lower(tx.to)).toBe(lower(AQUA));
    expect(tx.value).toBe(0n);
    const { functionName, args } = decodeFunctionData({ abi: ABI.AQUA_ABI, data: tx.data });
    const a = args as readonly unknown[];
    expect(functionName).toBe("push");
    expect(lower(a[0])).toBe(lower(MAKER));
    expect(lower(a[1])).toBe(lower(APP));
    expect(lower(a[2])).toBe(lower(built.strategyHash));
    expect(lower(a[3])).toBe(lower(WETH));
    expect(a[4]).toBe(5n);
  });
});

describe("strategyHash consistency", () => {
  it("Aqua's calculateStrategyHash equals the builder's strategyHash", () => {
    const viaAqua = AquaProtocolContract.calculateStrategyHash(new HexString(built.order)).toString();
    expect(lower(viaAqua)).toBe(lower(built.strategyHash));
  });
});
