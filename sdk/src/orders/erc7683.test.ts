import { decodeAbiParameters, hashTypedData } from "viem";
import { describe, expect, it } from "vitest";

import { buildErc7683Order } from "./erc7683";

const address = (suffix: string) => `0x${suffix.padStart(40, "0")}` as const;

describe("ERC-7683 Permit2 order", () => {
    it("encodes the settler witness and signs the matching Permit2 payload", () => {
        const order = buildErc7683Order(
            {
                chainId: 31_337,
                settler: address("1"),
                permit2: address("2"),
            },
            {
                swapper: address("3"),
                tokenIn: address("4"),
                tokenOut: address("5"),
                amountIn: 1_000_000n,
                minAmountOut: 999_000_000_000_000_000n,
                executorFee: 500n,
                nonce: 42n,
                deadline: 4_000_000_000,
            },
        );
        const [decoded] = decodeAbiParameters(
            [
                {
                    type: "tuple",
                    components: [
                        { name: "settler", type: "address" },
                        { name: "user", type: "address" },
                        { name: "chainId", type: "uint256" },
                        { name: "inputToken", type: "address" },
                        { name: "inputAmount", type: "uint256" },
                        { name: "outputToken", type: "address" },
                        { name: "outputAmount", type: "uint256" },
                        { name: "recipient", type: "address" },
                        { name: "executorFee", type: "uint256" },
                        { name: "nonce", type: "uint256" },
                        { name: "deadline", type: "uint256" },
                    ],
                },
            ],
            order.encodedOrder,
        );

        expect(decoded).toMatchObject({
            settler: address("1"),
            user: address("3"),
            inputAmount: 1_000_000n,
            outputAmount: 999_000_000_000_000_000n,
            executorFee: 500n,
            nonce: 42n,
        });
        expect(order.approval).toMatchObject({
            token: address("4"),
            spender: address("2"),
            amount: 1_000_000n,
        });
        expect(() => hashTypedData(order.permit)).not.toThrow();
    });
});
