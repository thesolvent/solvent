import { createWalletClient, custom, hashTypedData } from "viem";
import { describe, expect, it } from "vitest";

import { buildSwapOrder } from "../../src/orders";

const ADDRESS = "0x1111111111111111111111111111111111111111";
const OUTPUT_TOKEN = "0x2222222222222222222222222222222222222222";

describe("swap wallet payload", () => {
    it("serializes nested integers for wallet RPC without changing the signed digest", async () => {
        const amount = 5_000n * 10n ** 18n;
        const { permit } = buildSwapOrder(
            {
                chainId: 31337,
                reactor: ADDRESS,
                permit2: ADDRESS,
                cosigner: ADDRESS,
            },
            {
                swapper: ADDRESS,
                tokenIn: ADDRESS,
                tokenOut: OUTPUT_TOKEN,
                amountIn: amount,
                minAmountOut: 4_950_000_000n,
                deadline: 2_000_000_000,
                nonce: 9_007_199_254_740_993n,
            },
        );
        const typed = permit;
        let requests = 0;
        const wallet = createWalletClient({
            account: ADDRESS,
            transport: custom({
                request: async ({ method, params }) => {
                    expect(method).toBe("eth_signTypedData_v4");
                    requests += 1;
                    const payload = JSON.parse(params[1]);
                    expect(payload.message.permitted.amount).toBe(
                        amount.toString(),
                    );
                    expect(payload.message.nonce).toBe("9007199254740993");
                    expect(payload.message.witness.baseInputStartAmount).toBe(
                        amount.toString(),
                    );
                    expect(
                        payload.message.witness.baseOutputs[0].startAmount,
                    ).toBe("4950000000");
                    expect(hashTypedData(payload)).toBe(hashTypedData(typed));
                    return "0x";
                },
            }),
        });
        await wallet.signTypedData(typed);
        expect(requests).toBe(1);
    });
});
