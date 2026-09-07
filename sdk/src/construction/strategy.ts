import {
    AquaPeggedAmmStrategy,
    AquaXYCAmmStrategy,
    Address as SdkAddress,
    MakerTraits,
    Order,
    instructions,
} from "@1inch/swap-vm-sdk";
import { formatUnits, parseUnits } from "viem";

import type { Address, Hex } from "../index";

const { Price } = instructions.concentrate;

export type TokenRef = { address: Address; decimals: number };
export type PeggedTokenInfo = TokenRef & { reserve: bigint };

/** The three on-chain curves the maker wizard produces. `In range` resolves to `concentrated`
 * via {@link bandToPrices}; `Pegged`'s band % resolves to `linearWidth` via the re-exported
 * `linearWidthFromSymmetricRangePercent`. `buildStrategy` only ever sees resolved on-chain params. */
export type StrategyParams =
    | { curve: "full-range" }
    | {
          curve: "concentrated";
          base: TokenRef;
          quote: TokenRef;
          priceMin: string;
          priceMax: string;
      }
    | {
          curve: "pegged";
          tokenA: PeggedTokenInfo;
          tokenB: PeggedTokenInfo;
          linearWidth: bigint;
      };

export interface BuiltStrategy {
    /** The SwapVM program bytes. */
    program: Hex;
    /** `keccak256(order)` — the on-chain strategy identifier (Aqua mode needs no domain). */
    strategyHash: Hex;
    /** The ABI-encoded Aqua order the maker ships. */
    order: Hex;
}

/** Build a maker strategy: a strategy program, its hash, and the ABI-encoded order to ship. */
export function buildStrategy(input: {
    maker: Address;
    strategy: StrategyParams;
    feeBps?: number;
}): BuiltStrategy {
    const base = curveBuilder(input.strategy);
    const built =
        input.feeBps === undefined ? base : base.withFeeTokenIn(input.feeBps);
    const program = built.build();
    const order = Order.new({
        maker: new SdkAddress(input.maker),
        traits: MakerTraits.default(),
        program,
    });
    return {
        program: program.toString() as Hex,
        strategyHash: order.hash().toString() as Hex,
        order: order.encode().toString() as Hex,
    };
}

function curveBuilder(
    s: StrategyParams,
): AquaXYCAmmStrategy | AquaPeggedAmmStrategy {
    switch (s.curve) {
        case "full-range":
            return AquaXYCAmmStrategy.new();
        case "concentrated": {
            const pair = {
                baseToken: {
                    address: new SdkAddress(s.base.address),
                    decimals: BigInt(s.base.decimals),
                },
                quoteToken: {
                    address: new SdkAddress(s.quote.address),
                    decimals: BigInt(s.quote.decimals),
                },
            };
            const a = Price.fromHuman(s.priceMin, pair).toSqrt();
            const b = Price.fromHuman(s.priceMax, pair).toSqrt();
            // Bounds must be ordered by the on-chain sqrt price, which base/quote orientation may invert.
            const [sqrtPriceMin, sqrtPriceMax] = a < b ? [a, b] : [b, a];
            return AquaXYCAmmStrategy.newConcentrate({
                sqrtPriceMin,
                sqrtPriceMax,
            });
        }
        case "pegged":
            return AquaPeggedAmmStrategy.new({
                tokenA: peggedToken(s.tokenA),
                tokenB: peggedToken(s.tokenB),
                linearWidth: s.linearWidth,
            });
    }
}

function peggedToken(t: PeggedTokenInfo) {
    return {
        address: new SdkAddress(t.address),
        decimals: t.decimals,
        reserve: t.reserve,
    };
}

/** The `In range` preset: a symmetric band `± halfWidthPct` around `mid`, as the human price
 * bounds `concentrated` expects. Computed in 1e18 fixed-point to avoid float drift. */
export function bandToPrices(
    mid: string,
    halfWidthPct: number,
): { priceMin: string; priceMax: string } {
    const scaled = parseUnits(mid, 18);
    const bps = BigInt(Math.round(halfWidthPct * 100));
    return {
        priceMin: formatUnits((scaled * (10_000n - bps)) / 10_000n, 18),
        priceMax: formatUnits((scaled * (10_000n + bps)) / 10_000n, 18),
    };
}
