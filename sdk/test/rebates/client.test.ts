import { anvil } from "viem/chains";
import { createClient, custom } from "viem";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { AppConfig, Rebate } from "../../src/client";
import { createRebateClient } from "../../src/rebates";

const wallet = vi.hoisted(() => ({
    currentBlock: vi.fn(),
    ensureAllowance: vi.fn(),
    sendTransaction: vi.fn(),
    confirmTransaction: vi.fn(),
}));
vi.mock("../../src/swap/wallet", () => ({
    createWalletSession: () => wallet,
}));

const EXECUTOR = "0x1111111111111111111111111111111111111111";
const FILLER = "0x2222222222222222222222222222222222222222";
const TOKEN_IN = "0x3333333333333333333333333333333333333333";
const TOKEN_OUT = "0x4444444444444444444444444444444444444444";
const REBATE_ID = `0x${"55".repeat(32)}`;
const config = {
    chain_id: 31337,
    filler: FILLER,
} as AppConfig;
const rebate = {
    id: REBATE_ID,
    status: "ready",
    maker: "0x6666666666666666666666666666666666666666",
    strategy_hash: `0x${"77".repeat(32)}`,
    token_in: TOKEN_IN,
    token_out: TOKEN_OUT,
    amount_in: "100",
    amount_out: "120",
    gross_surplus: "20",
    safe_gas_cost: "10",
    maker_rebate: "9",
    executor_profit: "1",
    deviation_bps: 100,
    allocations: [],
    to: FILLER,
    nonce: "1",
    deadline_block: 200,
    calldata: "0x12345678",
    published_at: 1,
} satisfies Rebate;
const clients = createClient({
    chain: anvil,
    transport: custom({ request: vi.fn() }),
});

function setup(current: Rebate = rebate) {
    const api = {
        config: vi.fn().mockResolvedValue(config),
        rebateDetail: vi.fn().mockResolvedValue(current),
    };
    return {
        api,
        rebates: createRebateClient({
            api,
            publicClient: clients,
            walletClient: clients,
        }),
    };
}

beforeEach(() => {
    vi.resetAllMocks();
    wallet.currentBlock.mockResolvedValue(100n);
    wallet.ensureAllowance.mockResolvedValue(undefined);
    wallet.sendTransaction.mockResolvedValue("0xtransaction");
    wallet.confirmTransaction.mockResolvedValue(undefined);
});

describe("rebate intent", () => {
    it("approves the contract deposit and confirms one immutable execution", async () => {
        const { rebates } = setup();
        const intent = rebates.createIntent({
            executor: EXECUTOR,
            rebateId: REBATE_ID,
        });

        const first = intent.submit();
        expect(intent.submit()).toBe(first);
        await expect(first).resolves.toEqual({
            rebateId: REBATE_ID,
            transactionHash: "0xtransaction",
        });
        await intent.submit();

        expect(wallet.ensureAllowance).toHaveBeenCalledWith({
            owner: EXECUTOR,
            chainId: 31337,
            token: TOKEN_IN,
            spender: FILLER,
            amount: 109n,
        });
        expect(wallet.sendTransaction).toHaveBeenCalledOnce();
        expect(wallet.confirmTransaction).toHaveBeenCalledOnce();
    });

    it("never approves work that is executed, expired, or targets another contract", async () => {
        const cases: Rebate[] = [
            { ...rebate, status: "executed", to: undefined },
            { ...rebate, deadline_block: 100 },
            {
                ...rebate,
                to: "0x8888888888888888888888888888888888888888",
            },
        ];

        for (const work of cases) {
            const { rebates } = setup(work);
            await expect(
                rebates
                    .createIntent({ executor: EXECUTOR, rebateId: REBATE_ID })
                    .submit(),
            ).rejects.toThrow();
        }
        expect(wallet.ensureAllowance).not.toHaveBeenCalled();
        expect(wallet.sendTransaction).not.toHaveBeenCalled();
    });
});
