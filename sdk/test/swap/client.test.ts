import { anvil } from "viem/chains";
import { createClient, custom } from "viem";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
    createSwapClient,
    SwapDeclinedError,
    type SwapSubmissionStatus,
} from "../../src/swap";
import type { AppConfig } from "../../src/client";

const sign = vi.hoisted(() => vi.fn());
vi.mock("../../src/swap/wallet", () => ({
    createWalletSession: () => ({ sign, tokenAccount: vi.fn() }),
}));
const ADDRESS = "0x1111111111111111111111111111111111111111";
const TOKEN_IN = "0x2222222222222222222222222222222222222222";
const TOKEN_OUT = "0x3333333333333333333333333333333333333333";
const REACTOR = "0x4444444444444444444444444444444444444444";
const PERMIT2 = "0x5555555555555555555555555555555555555555";
const COSIGNER = "0x6666666666666666666666666666666666666666";
const config = {
    chain_id: 31337,
    reactor: REACTOR,
    permit2: PERMIT2,
    cosigner: COSIGNER,
} as AppConfig;
const terms = {
    swapper: ADDRESS,
    tokenIn: TOKEN_IN,
    tokenOut: TOKEN_OUT,
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

    it("reports preparation, signing, and submission to the caller", async () => {
        const { swaps } = setup();
        const statuses: SwapSubmissionStatus[] = [];
        sign.mockImplementationOnce(async (_order, options) => {
            options?.onStatus?.({ kind: "signing" });
            return "0xsigned";
        });

        await swaps
            .createIntent(terms)
            .submit({ onStatus: (status) => statuses.push(status) });

        expect(statuses).toEqual([
            { kind: "preparing" },
            { kind: "signing" },
            { kind: "submitting" },
        ]);
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

    it("does not retry cached signed bytes after their deadline", async () => {
        vi.useFakeTimers();
        try {
            const now = new Date("2026-09-09T10:00:00Z");
            vi.setSystemTime(now);
            const { api, swaps } = setup();
            api.swap.mockRejectedValueOnce(new Error("Response lost"));
            const intent = swaps.createIntent({
                ...terms,
                deadline: Math.floor(now.getTime() / 1_000) + 1,
            });
            await expect(intent.submit()).rejects.toThrow("Response lost");

            vi.setSystemTime(new Date(now.getTime() + 2_000));
            await expect(intent.submit()).rejects.toThrow("Swap order expired");
            expect(api.swap).toHaveBeenCalledOnce();
        } finally {
            vi.useRealTimers();
        }
    });
});
