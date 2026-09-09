import { anvil } from "viem/chains";
import { createClient, custom, maxUint256, type Address, type Hex } from "viem";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { AppConfig } from "../../src/client";
import { Strategy } from "../../src/construction";
import {
    createPositionClient,
    PositionExistsError,
    PositionPreviewError,
} from "../../src/positions";

const wallet = vi.hoisted(() => ({
    ensureAllowance: vi.fn(),
    sendTransaction: vi.fn(),
    confirmTransaction: vi.fn(),
}));
vi.mock("../../src/swap/wallet", () => ({
    createWalletSession: () => wallet,
}));

const MAKER = "0x1111111111111111111111111111111111111111" as Address;
const AQUA = "0x2222222222222222222222222222222222222222" as Address;
const APP = "0x3333333333333333333333333333333333333333" as Address;
const TOKEN_A = "0x4444444444444444444444444444444444444444" as Address;
const TOKEN_B = "0x5555555555555555555555555555555555555555" as Address;
const HASH = `0x${"ab".repeat(32)}` as Hex;
const config = {
    chain_id: 31337,
    aqua: AQUA,
    app: APP,
} as AppConfig;
const strategy = Strategy.fullRange().salt(7n).build(MAKER);
const amounts = [
    { token: TOKEN_A, amount: 10n },
    { token: TOKEN_B, amount: 20n },
];
const viemClient = createClient({
    chain: anvil,
    transport: custom({ request: vi.fn() }),
});

function setup(preview = {}) {
    const api = {
        config: vi.fn().mockResolvedValue(config),
        positionsPreview: vi.fn().mockResolvedValue({
            exists: false,
            requires_approval: [TOKEN_B],
            warnings: [],
            ...preview,
        }),
    };
    const positions = createPositionClient({
        api,
        publicClient: viemClient,
        walletClient: viemClient,
    });
    return { api, positions };
}

beforeEach(() => {
    vi.resetAllMocks();
    wallet.ensureAllowance.mockResolvedValue(undefined);
    wallet.sendTransaction.mockResolvedValue(HASH);
    wallet.confirmTransaction.mockResolvedValue(undefined);
});

describe("position creation intent", () => {
    const malformedAmounts = [
        [],
        [{ token: TOKEN_A, amount: 1n }],
        [
            { token: TOKEN_A, amount: 1n },
            { token: TOKEN_A, amount: 2n },
        ],
        [
            { token: TOKEN_A, amount: 0n },
            { token: TOKEN_B, amount: 2n },
        ],
        [
            { token: TOKEN_A, amount: 1n << 248n },
            { token: TOKEN_B, amount: 2n },
        ],
    ].map((amounts) => ({ amounts }));

    it.each(malformedAmounts)("rejects malformed Aqua reserves before preview", async ({ amounts: badAmounts }) => {
        const { api, positions } = setup();
        await expect(
            positions
                .createIntent({ maker: MAKER, strategy, amounts: badAmounts })
                .submit(),
        ).rejects.toThrow();
        expect(api.positionsPreview).not.toHaveBeenCalled();
        expect(wallet.ensureAllowance).not.toHaveBeenCalled();
        expect(wallet.sendTransaction).not.toHaveBeenCalled();
    });

    it("rejects a maker different from the one encoded in the strategy", async () => {
        const { api, positions } = setup();

        await expect(
            positions.createIntent({ maker: AQUA, strategy, amounts }).submit(),
        ).rejects.toThrow(
            "Position maker must match the maker encoded in the strategy",
        );
        expect(api.positionsPreview).not.toHaveBeenCalled();
        expect(wallet.ensureAllowance).not.toHaveBeenCalled();
        expect(wallet.sendTransaction).not.toHaveBeenCalled();
    });

    it("previews, approves only deficient tokens, and confirms one ship", async () => {
        const { api, positions } = setup();

        await expect(
            positions.createIntent({ maker: MAKER, strategy, amounts }).submit(),
        ).resolves.toEqual({ strategyHash: strategy.strategyHash, transactionHash: HASH });

        expect(api.positionsPreview).toHaveBeenCalledWith({
            maker: MAKER,
            strategy_hash: strategy.strategyHash,
            amounts: amounts.map(({ token, amount }) => ({
                token,
                amount: amount.toString(),
            })),
        });
        expect(wallet.ensureAllowance).toHaveBeenCalledOnce();
        expect(wallet.ensureAllowance).toHaveBeenCalledWith({
            owner: MAKER,
            chainId: 31337,
            token: TOKEN_B,
            spender: AQUA,
            amount: 20n,
            approvalAmount: maxUint256,
        });
        expect(wallet.sendTransaction).toHaveBeenCalledOnce();
        expect(wallet.sendTransaction.mock.calls[0][0]).toMatchObject({
            owner: MAKER,
            chainId: 31337,
            to: AQUA,
        });
        expect(wallet.confirmTransaction).toHaveBeenCalledWith(HASH);
    });

    it.each([
        [{ exists: true }, PositionExistsError],
        [{ warnings: ["insufficient DAI balance"] }, PositionPreviewError],
        [
            {
                requires_approval: [
                    "0x6666666666666666666666666666666666666666",
                ],
            },
            PositionPreviewError,
        ],
    ])("stops before wallet prompts when preview rejects the ship", async (preview, ErrorType) => {
        const { positions } = setup(preview);

        await expect(
            positions.createIntent({ maker: MAKER, strategy, amounts }).submit(),
        ).rejects.toBeInstanceOf(ErrorType);
        expect(wallet.ensureAllowance).not.toHaveBeenCalled();
        expect(wallet.sendTransaction).not.toHaveBeenCalled();
    });

    it("reuses its transaction hash when receipt confirmation is retried", async () => {
        wallet.confirmTransaction.mockRejectedValueOnce(new Error("receipt timeout"));
        const { api, positions } = setup();
        const intent = positions.createIntent({ maker: MAKER, strategy, amounts });

        await expect(intent.submit()).rejects.toThrow("receipt timeout");
        await expect(intent.submit()).resolves.toEqual({
            strategyHash: strategy.strategyHash,
            transactionHash: HASH,
        });

        expect(api.positionsPreview).toHaveBeenCalledOnce();
        expect(wallet.ensureAllowance).toHaveBeenCalledOnce();
        expect(wallet.sendTransaction).toHaveBeenCalledOnce();
        expect(wallet.confirmTransaction).toHaveBeenCalledTimes(2);
        expect(wallet.confirmTransaction).toHaveBeenNthCalledWith(1, HASH);
        expect(wallet.confirmTransaction).toHaveBeenNthCalledWith(2, HASH);
    });

    it("rechecks authorization when a ship was never broadcast", async () => {
        wallet.sendTransaction.mockRejectedValueOnce(new Error("wallet rejected"));
        const { api, positions } = setup();
        const intent = positions.createIntent({ maker: MAKER, strategy, amounts });

        await expect(intent.submit()).rejects.toThrow("wallet rejected");
        await expect(intent.submit()).resolves.toEqual({
            strategyHash: strategy.strategyHash,
            transactionHash: HASH,
        });

        expect(api.positionsPreview).toHaveBeenCalledOnce();
        expect(wallet.ensureAllowance).toHaveBeenCalledTimes(2);
        expect(wallet.sendTransaction).toHaveBeenCalledTimes(2);
        expect(wallet.confirmTransaction).toHaveBeenCalledOnce();
    });

    it("retries a transient preflight failure without changing the intent", async () => {
        const { api, positions } = setup();
        api.positionsPreview.mockRejectedValueOnce(new Error("API unavailable"));
        const intent = positions.createIntent({ maker: MAKER, strategy, amounts });

        await expect(intent.submit()).rejects.toThrow("API unavailable");
        await expect(intent.submit()).resolves.toEqual({
            strategyHash: strategy.strategyHash,
            transactionHash: HASH,
        });

        expect(api.positionsPreview).toHaveBeenCalledTimes(2);
        expect(wallet.sendTransaction).toHaveBeenCalledOnce();
    });

    it("coalesces concurrent submissions into one wallet flow", async () => {
        let confirm!: () => void;
        wallet.confirmTransaction.mockReturnValue(
            new Promise<void>((resolve) => {
                confirm = resolve;
            }),
        );
        const { positions } = setup();
        const intent = positions.createIntent({ maker: MAKER, strategy, amounts });

        const first = intent.submit();
        const second = intent.submit();
        expect(first).toBe(second);
        await vi.waitFor(() => expect(wallet.confirmTransaction).toHaveBeenCalledOnce());
        confirm();
        await expect(Promise.all([first, second])).resolves.toHaveLength(2);

        expect(wallet.ensureAllowance).toHaveBeenCalledOnce();
        expect(wallet.sendTransaction).toHaveBeenCalledOnce();
    });
});

describe("position management intents", () => {
    it("checks push funding, approves Aqua, simulates, broadcasts, and confirms", async () => {
        const { api, positions } = setup();

        await expect(
            positions
                .pushIntent({
                    maker: MAKER,
                    strategyHash: HASH,
                    token: TOKEN_A,
                    amount: 25n,
                })
                .submit(),
        ).resolves.toEqual({ transactionHash: HASH });

        expect(api.config).toHaveBeenCalledOnce();
        expect(api.positionsPreview).not.toHaveBeenCalled();
        expect(wallet.ensureAllowance).toHaveBeenCalledWith({
            owner: MAKER,
            chainId: 31337,
            token: TOKEN_A,
            spender: AQUA,
            amount: 25n,
            approvalAmount: maxUint256,
        });
        expect(wallet.sendTransaction).toHaveBeenCalledWith(
            expect.objectContaining({
                owner: MAKER,
                chainId: 31337,
                to: AQUA,
            }),
        );
        expect(wallet.confirmTransaction).toHaveBeenCalledWith(HASH);
    });

    it("simulates, broadcasts, and confirms a dock without token approvals", async () => {
        const { api, positions } = setup();

        await expect(
            positions
                .dockIntent({
                    maker: MAKER,
                    strategyHash: HASH,
                    tokens: [TOKEN_A, TOKEN_B],
                })
                .submit(),
        ).resolves.toEqual({ transactionHash: HASH });

        expect(api.config).toHaveBeenCalledOnce();
        expect(api.positionsPreview).not.toHaveBeenCalled();
        expect(wallet.ensureAllowance).not.toHaveBeenCalled();
        expect(wallet.sendTransaction).toHaveBeenCalledWith(
            expect.objectContaining({
                owner: MAKER,
                chainId: 31337,
                to: AQUA,
            }),
        );
        expect(wallet.confirmTransaction).toHaveBeenCalledWith(HASH);
    });

    it("reuses a management transaction when receipt confirmation is retried", async () => {
        wallet.confirmTransaction.mockRejectedValueOnce(
            new Error("receipt timeout"),
        );
        const { positions } = setup();
        const intent = positions.dockIntent({
            maker: MAKER,
            strategyHash: HASH,
            tokens: [TOKEN_A, TOKEN_B],
        });

        await expect(intent.submit()).rejects.toThrow("receipt timeout");
        await expect(intent.submit()).resolves.toEqual({
            transactionHash: HASH,
        });

        expect(wallet.sendTransaction).toHaveBeenCalledOnce();
        expect(wallet.confirmTransaction).toHaveBeenCalledTimes(2);
    });

    it("rejects invalid actions before reading deployment config", async () => {
        const { api, positions } = setup();

        await expect(
            positions
                .pushIntent({
                    maker: MAKER,
                    strategyHash: HASH,
                    token: TOKEN_A,
                    amount: 0n,
                })
                .submit(),
        ).rejects.toThrow("push amount must be greater than zero");
        await expect(
            positions
                .dockIntent({
                    maker: MAKER,
                    strategyHash: HASH,
                    tokens: [TOKEN_A, TOKEN_A],
                })
                .submit(),
        ).rejects.toThrow(
            "position tokens must contain distinct token addresses",
        );

        expect(api.config).not.toHaveBeenCalled();
        expect(wallet.sendTransaction).not.toHaveBeenCalled();
    });
});
