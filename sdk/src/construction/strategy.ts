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

export interface BuiltStrategy {
    /** The SwapVM program bytes. */
    program: Hex;
    /** `keccak256(order)` — the on-chain strategy identifier (Aqua mode needs no domain). */
    strategyHash: Hex;
    /** The ABI-encoded Aqua order the maker ships. */
    order: Hex;
}

type SdkBuilder = AquaXYCAmmStrategy | AquaPeggedAmmStrategy;

/** A maker strategy, built fluently to mirror the underlying `@1inch/swap-vm-sdk` builders.
 * Pick a curve, optionally add a fee, then `build(maker)`:
 *
 * ```ts
 * Strategy.concentrated({ base, quote, priceMin: "0.5", priceMax: "2" }).fee(30).build(maker);
 * Strategy.inRange({ base, quote, mid: "3000", halfWidthPct: 5 }).build(maker);
 * Strategy.pegged({ tokenA, tokenB, linearWidth }).build(maker);
 * ```
 *
 * Instances are immutable — `fee` and `salt` return a new `Strategy`. */
export class Strategy {
    private constructor(
        private readonly resolve: () => SdkBuilder,
        private readonly feeBps?: number,
    ) {}

    /** Full-range constant-product (XYC). */
    static fullRange(): Strategy {
        return new Strategy(() => AquaXYCAmmStrategy.new());
    }

    /** Concentrated liquidity between explicit human price bounds (quote per 1 base). */
    static concentrated(p: {
        base: TokenRef;
        quote: TokenRef;
        priceMin: string;
        priceMax: string;
    }): Strategy {
        return new Strategy(() =>
            concentrate(p.base, p.quote, p.priceMin, p.priceMax),
        );
    }

    /** Concentrated liquidity in a symmetric band `± halfWidthPct` around `mid`. */
    static inRange(p: {
        base: TokenRef;
        quote: TokenRef;
        mid: string;
        halfWidthPct: number;
    }): Strategy {
        const { priceMin, priceMax } = bandToPrices(p.mid, p.halfWidthPct);
        return new Strategy(() =>
            concentrate(p.base, p.quote, priceMin, priceMax),
        );
    }

    /** Pegged (stable) curve; `linearWidth` sets the band (see `linearWidthFromSymmetricRangePercent`). */
    static pegged(p: {
        tokenA: PeggedTokenInfo;
        tokenB: PeggedTokenInfo;
        linearWidth: bigint;
    }): Strategy {
        return new Strategy(() =>
            AquaPeggedAmmStrategy.new({
                tokenA: peggedToken(p.tokenA),
                tokenB: peggedToken(p.tokenB),
                linearWidth: p.linearWidth,
            }),
        );
    }

    /** A maker fee, in bps, taken on the input token. */
    fee(bps: number): Strategy {
        return new Strategy(this.resolve, bps);
    }

    /** Give an otherwise identical position a distinct identity; zero keeps the unsalted program. */
    salt(value: bigint): Strategy {
        return new Strategy(() => this.resolve().withSalt(value), this.feeBps);
    }

    /** Encode for `maker`: the program, its hash, and the order to ship. */
    build(maker: Address): BuiltStrategy {
        const base = this.resolve();
        const built =
            this.feeBps === undefined ? base : base.withFeeTokenIn(this.feeBps);
        const program = built.build();
        const order = Order.new({
            maker: new SdkAddress(maker),
            traits: MakerTraits.default(),
            program,
        });
        return {
            program: program.toString() as Hex,
            strategyHash: order.hash().toString() as Hex,
            order: order.encode().toString() as Hex,
        };
    }
}

function concentrate(
    base: TokenRef,
    quote: TokenRef,
    priceMin: string,
    priceMax: string,
): AquaXYCAmmStrategy {
    const pair = {
        baseToken: {
            address: new SdkAddress(base.address),
            decimals: BigInt(base.decimals),
        },
        quoteToken: {
            address: new SdkAddress(quote.address),
            decimals: BigInt(quote.decimals),
        },
    };
    const a = Price.fromHuman(priceMin, pair).toSqrt();
    const b = Price.fromHuman(priceMax, pair).toSqrt();
    // Bounds must be ordered by the on-chain sqrt price, which base/quote orientation may invert.
    const [sqrtPriceMin, sqrtPriceMax] = a < b ? [a, b] : [b, a];
    return AquaXYCAmmStrategy.newConcentrate({ sqrtPriceMin, sqrtPriceMax });
}

function peggedToken(t: PeggedTokenInfo) {
    return {
        address: new SdkAddress(t.address),
        decimals: t.decimals,
        reserve: t.reserve,
    };
}

/** A symmetric band `± halfWidthPct` around `mid`, as the human price bounds `concentrated` expects.
 * Computed in 1e18 fixed-point to avoid float drift. */
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
