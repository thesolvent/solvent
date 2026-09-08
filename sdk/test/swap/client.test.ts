import { anvil } from "viem/chains";
import { createClient, custom } from "viem";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createSwapClient, SwapDeclinedError } from "../../src/swap";
import type { AppConfig } from "../../src/client";

const sign = vi.hoisted(() => vi.fn());
vi.mock("../../src/swap/wallet", () => ({
    createWalletSession: () => ({ sign, tokenAccount: vi.fn() }),
}));
const ADDRESS = "0x1111111111111111111111111111111111111111";
const config = {
    chain_id: 31337,
    reactor: ADDRESS,
    permit2: ADDRESS,
    cosigner: ADDRESS,
} as AppConfig;
const terms = {
    swapper: ADDRESS,
    tokenIn: ADDRESS,
    tokenOut: ADDRESS,
    amountIn: 10n,
    minAmountOut: 9n,
    deadline: 2_000_000_000,
};
const client = createClient({
    chain: anvil,
    transport: custom({ request: vi.fn() }),
});
function setup() {
    const api = {
        config: vi.fn().mockResolvedValue(config),
        swap: vi
            .fn()
            .mockResolvedValue({ trade_id: "trade", status: "submitted" }),
    };
    const swaps = createSwapClient({
        api,
        publicClient: client,
        walletClient: client,
    });
    return { api, swaps };
}
beforeEach(() => {
    sign.mockReset().mockResolvedValue("0xsigned");
});

describe("swap intent", () => {
    it("reuses the signed authorization after an ambiguous HTTP failure", async () => {
        const { api, swaps } = setup();
        api.swap.mockRejectedValueOnce(new Error("Response lost"));
        const intent = swaps.createIntent(terms);
        await expect(intent.submit()).rejects.toThrow("Response lost");
        await expect(intent.submit()).resolves.toMatchObject({
            status: "submitted",
        });
        expect(sign).toHaveBeenCalledOnce();
        expect(api.config).toHaveBeenCalledOnce();
        expect(api.swap.mock.calls[1][0]).toBe(api.swap.mock.calls[0][0]);
    });

    it("shares concurrent work and snapshots the authorized terms", async () => {
        const { api, swaps } = setup();
        const editable = { ...terms };
        const intent = swaps.createIntent(editable);
        editable.amountIn = 100n;
        const first = intent.submit();
        expect(intent.submit()).toBe(first);
        await first;
        await intent.submit();
        expect(sign.mock.calls[0][0].approval.amount).toBe(10n);
        expect(sign).toHaveBeenCalledOnce();
        expect(api.swap).toHaveBeenCalledOnce();
    });

    it("retains a known decline without creating or posting another order", async () => {
        const { api, swaps } = setup();
        api.swap.mockResolvedValue({ trade_id: "trade", status: "declined" });
        const intent = swaps.createIntent(terms);
        await expect(intent.submit()).rejects.toBeInstanceOf(SwapDeclinedError);
        await expect(intent.submit()).rejects.toBeInstanceOf(SwapDeclinedError);
        expect(sign).toHaveBeenCalledOnce();
        expect(api.swap).toHaveBeenCalledOnce();
    });
});
