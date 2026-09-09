import { describe, expect, it } from "vitest";

import { buildSwapOrder, type OrderTerms, type OrderVenue } from "../../src/orders";
import { InputValidationError, MAX_UINT256 } from "../../src/validation";

const venue: OrderVenue = {
    chainId: 31337,
    reactor: "0x1111111111111111111111111111111111111111",
    permit2: "0x2222222222222222222222222222222222222222",
    cosigner: "0x3333333333333333333333333333333333333333",
};
const terms: OrderTerms = {
    swapper: "0x4444444444444444444444444444444444444444",
    tokenIn: "0x5555555555555555555555555555555555555555",
    tokenOut: "0x6666666666666666666666666666666666666666",
    amountIn: 10n,
    minAmountOut: 9n,
    deadline: 2_000_000_000,
    nonce: 0n,
};

describe("swap order boundaries", () => {
    it("builds at the uint256 nonce boundary", () => {
        expect(buildSwapOrder(venue, { ...terms, nonce: MAX_UINT256 })).toMatchObject({
            deadline: terms.deadline,
            approval: { amount: terms.amountIn },
        });
    });

    it.each([
        [{ ...venue, chainId: 0 }, terms],
        [{ ...venue, reactor: "0x0" }, terms],
        [venue, { ...terms, tokenOut: terms.tokenIn }],
        [venue, { ...terms, amountIn: 0n }],
        [venue, { ...terms, minAmountOut: 0n }],
        [venue, { ...terms, nonce: MAX_UINT256 + 1n }],
        [venue, { ...terms, deadline: 1 }],
    ] as const)("rejects an unusable venue or order", (badVenue, badTerms) => {
        expect(() => buildSwapOrder(badVenue, badTerms)).toThrow(InputValidationError);
    });
});
