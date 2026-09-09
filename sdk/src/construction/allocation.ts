import { Address as SdkAddress, instructions } from "@1inch/swap-vm-sdk";
import type { Address } from "viem";

import type { TokenRef } from "./strategy";
import {
    assertDistinctAddresses,
    decimalValuesEqual,
    InputValidationError,
    validatedAddress,
    validatedPositiveDecimal,
    validatedTokenDecimals,
} from "../validation";

export type AllocationCurve =
    | { kind: "fullRange" }
    | { kind: "concentrated"; priceMin: string; priceMax: string }
    | { kind: "pegged" };

export type ReserveAllocation = { base: bigint; quote: bigint };

export interface StrategyAllocator {
    fromBase(reserve: bigint): ReserveAllocation;
    fromQuote(reserve: bigint): ReserveAllocation;
    matches(allocation: ReserveAllocation): boolean;
    max(available: ReserveAllocation): ReserveAllocation;
}

/** Computes the reserves whose initial marginal price matches `spotPrice`. */
export function strategyAllocator(input: {
    base: TokenRef;
    quote: TokenRef;
    spotPrice: string;
    curve: AllocationCurve;
}): StrategyAllocator {
    const baseAddress = validatedAddress(input.base.address, "base token");
    const quoteAddress = validatedAddress(input.quote.address, "quote token");
    assertDistinctAddresses(baseAddress, quoteAddress, "token pair");
    validatedTokenDecimals(input.base.decimals, "base token decimals");
    validatedTokenDecimals(input.quote.decimals, "quote token decimals");
    validatedPositiveDecimal(input.spotPrice, "spot price");
    if (input.curve.kind === "concentrated") {
        validatedPositiveDecimal(input.curve.priceMin, "minimum price");
        validatedPositiveDecimal(input.curve.priceMax, "maximum price");
        if (decimalValuesEqual(input.curve.priceMin, input.curve.priceMax)) {
            throw new InputValidationError(
                "price range",
                "out_of_range",
                "Minimum and maximum prices must be different",
            );
        }
    }
    const baseToken = sdkToken(input.base);
    const quoteToken = sdkToken(input.quote);

    switch (input.curve.kind) {
        case "fullRange":
            return marginalPriceAllocator(
                baseToken,
                quoteToken,
                input.spotPrice,
            );
        case "concentrated": {
            const pair = pricePair(input.base, input.quote);
            const range = instructions.concentrate.PriceRange.new({
                minPrice: instructions.concentrate.Price.fromHuman(
                    input.curve.priceMin,
                    pair,
                ),
                spotPrice: instructions.concentrate.Price.fromHuman(
                    input.spotPrice,
                    pair,
                ),
                maxPrice: instructions.concentrate.Price.fromHuman(
                    input.curve.priceMax,
                    pair,
                ),
            });
            const fromBase = (reserve: bigint) =>
                concentratedAllocation(
                    range,
                    baseToken.address,
                    reserve,
                    baseToken,
                );
            const fromQuote = (reserve: bigint) =>
                concentratedAllocation(
                    range,
                    quoteToken.address,
                    reserve,
                    baseToken,
                );
            return createAllocator(fromBase, fromQuote, (available) => {
                const allocation = range.computeMaxAllocation({
                    reserveA: instructions.concentrate.TokenReserve.new({
                        token: baseToken.address,
                        reserve: available.base,
                    }),
                    reserveB: instructions.concentrate.TokenReserve.new({
                        token: quoteToken.address,
                        reserve: available.quote,
                    }),
                });
                return sortedAllocation(allocation, baseToken.address);
            });
        }
        case "pegged":
            return marginalPriceAllocator(
                baseToken,
                quoteToken,
                input.spotPrice,
            );
    }
}

type Allocate = (reserve: bigint) => ReserveAllocation;

function createAllocator(
    fromBase: Allocate,
    fromQuote: Allocate,
    maxAllocation?: (available: ReserveAllocation) => ReserveAllocation,
): StrategyAllocator {
    const allocate = (fn: Allocate, reserve: bigint) => {
        if (reserve < 0n) throw new RangeError("reserve must not be negative");
        return reserve === 0n ? { base: 0n, quote: 0n } : fn(reserve);
    };
    const fromBaseChecked = (reserve: bigint) => allocate(fromBase, reserve);
    const fromQuoteChecked = (reserve: bigint) => allocate(fromQuote, reserve);

    return {
        fromBase: fromBaseChecked,
        fromQuote: fromQuoteChecked,
        matches(allocation) {
            if (allocation.base < 0n || allocation.quote < 0n) return false;
            return (
                fromBaseChecked(allocation.base).quote === allocation.quote ||
                fromQuoteChecked(allocation.quote).base === allocation.base
            );
        },
        max(available) {
            if (available.base < 0n || available.quote < 0n) {
                throw new RangeError("available reserves must not be negative");
            }
            if (available.base === 0n || available.quote === 0n) {
                return { base: 0n, quote: 0n };
            }
            if (maxAllocation) return maxAllocation(available);
            const baseLimited = fromBaseChecked(available.base);
            return baseLimited.quote <= available.quote
                ? baseLimited
                : fromQuoteChecked(available.quote);
        },
    };
}

function concentratedAllocation(
    range: ReturnType<typeof instructions.concentrate.PriceRange.new>,
    fixedToken: SdkAddress,
    fixedReserve: bigint,
    base: ReturnType<typeof sdkToken>,
): ReserveAllocation {
    return sortedAllocation(
        range.computeFixedAllocation(
            instructions.concentrate.TokenReserve.new({
                token: fixedToken,
                reserve: fixedReserve,
            }),
        ),
        base.address,
    );
}

function fixedAllocation(
    calculator: ReturnType<
        typeof instructions.peggedSwap.PeggedSwapCalculator.new
    >,
    price: ReturnType<typeof instructions.peggedSwap.PeggedPrice.fromHuman>,
    fixedToken: SdkAddress,
    fixedReserve: bigint,
    base: SdkAddress,
): ReserveAllocation {
    const allocation = calculator.computeFixedAllocation(
        price,
        fixedToken,
        fixedReserve,
    );
    return calculator.tokenLt.address.equal(base)
        ? { base: allocation.reserveLt, quote: allocation.reserveGt }
        : { base: allocation.reserveGt, quote: allocation.reserveLt };
}

function marginalPriceAllocator(
    base: ReturnType<typeof sdkToken>,
    quote: ReturnType<typeof sdkToken>,
    spotPrice: string,
): StrategyAllocator {
    const price = instructions.peggedSwap.PeggedPrice.fromHuman(spotPrice, {
        baseToken: base,
        quoteToken: quote,
    });
    const calculator = instructions.peggedSwap.PeggedSwapCalculator.new({
        tokenA: base,
        tokenB: quote,
    });
    // These curves encode their initial marginal price in their reserve ratio. This calculator
    // preserves SwapVM token ordering and decimal normalization for both directions.
    return createAllocator(
        (reserve) =>
            fixedAllocation(
                calculator,
                price,
                base.address,
                reserve,
                base.address,
            ),
        (reserve) =>
            fixedAllocation(
                calculator,
                price,
                quote.address,
                reserve,
                base.address,
            ),
    );
}

function sortedAllocation(
    allocation: {
        reserve0: { token: SdkAddress; reserve: bigint };
        reserve1: { token: SdkAddress; reserve: bigint };
    },
    base: SdkAddress,
): ReserveAllocation {
    return allocation.reserve0.token.equal(base)
        ? {
              base: allocation.reserve0.reserve,
              quote: allocation.reserve1.reserve,
          }
        : {
              base: allocation.reserve1.reserve,
              quote: allocation.reserve0.reserve,
          };
}

function sdkToken(token: TokenRef) {
    return {
        address: new SdkAddress(token.address),
        decimals: token.decimals,
    };
}

function pricePair(base: TokenRef, quote: TokenRef) {
    return {
        baseToken: { ...sdkToken(base), decimals: BigInt(base.decimals) },
        quoteToken: { ...sdkToken(quote), decimals: BigInt(quote.decimals) },
    };
}
