import { anvil } from "viem/chains";
import { createClient, custom, maxUint256, type Address } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { OrderApproval, UnsignedOrder } from "../../src/orders";
import { createWalletSession } from "../../src/swap/wallet";

const rpc = vi.hoisted(() => ({
    accounts: vi.fn(),
    chain: vi.fn(),
    read: vi.fn(),
    call: vi.fn(),
    write: vi.fn(),
    receipt: vi.fn(),
    sign: vi.fn(),
    send: vi.fn(),
}));
vi.mock("viem/actions", async (original) => ({
    ...(await original<typeof import("viem/actions")>()),
    getAddresses: rpc.accounts,
    getChainId: rpc.chain,
    getBlockNumber: async () => 1n,
    readContract: rpc.read,
    call: rpc.call,
    writeContract: rpc.write,
    waitForTransactionReceipt: rpc.receipt,
    signTypedData: rpc.sign,
    sendTransaction: rpc.send,
}));
const client = createClient({
    chain: anvil,
    transport: custom({ request: vi.fn() }),
});
const session = createWalletSession({
    publicClient: client,
    walletClient: client,
});
const approval: OrderApproval = {
    owner: "0x1111111111111111111111111111111111111111",
    chainId: 31337,
    token: "0x2222222222222222222222222222222222222222",
    spender: "0x3333333333333333333333333333333333333333",
    amount: 10n,
};
const order: UnsignedOrder = {
    approval,
    encodedOrder: "0x",
    deadline: 2_000_000_000,
    permit: {
        domain: {},
        types: {},
        primaryType: "PermitWitnessTransferFrom",
        message: {},
    },
};

beforeEach(() => {
    vi.resetAllMocks();
    rpc.accounts.mockResolvedValue([approval.owner]);
    rpc.chain.mockResolvedValue(31337);
    rpc.read.mockImplementation(async (_, { functionName }) =>
        functionName === "balanceOf" ? 100n : 10n,
    );
    rpc.call.mockResolvedValue({ data: "0x" });
    rpc.write.mockResolvedValue("0xapproval");
    rpc.receipt.mockResolvedValue({ status: "success" });
    rpc.sign.mockResolvedValue("0xsigned");
    rpc.send.mockResolvedValue("0xsent");
});

describe("token approval orchestration", () => {
    it("checks the required balance while granting a reusable allowance", async () => {
        rpc.read
            .mockResolvedValueOnce(100n)
            .mockResolvedValueOnce(0n)
            .mockResolvedValueOnce(100n)
            .mockResolvedValueOnce(maxUint256);

        await session.ensureAllowance({
            ...approval,
            approvalAmount: maxUint256,
        });

        expect(rpc.write.mock.calls[0][1].args).toEqual([
            approval.spender,
            maxUint256,
        ]);
    });

    it.each([
        { allowance: 0n, writes: [10n] },
        { allowance: 3n, writes: [0n, 10n] },
        { allowance: 10n, writes: [] },
    ])(
        "reuses or safely increases allowance $allowance",
        async ({ allowance, writes }) => {
            rpc.read
                .mockResolvedValueOnce(100n)
                .mockResolvedValueOnce(allowance);
            for (const value of writes) {
                rpc.read
                    .mockResolvedValueOnce(100n)
                    .mockResolvedValueOnce(value);
            }
            await session.sign(order);
            expect(rpc.write.mock.calls.map(([, req]) => req.args)).toEqual(
                writes.map((value) => [approval.spender, value]),
            );
            expect(rpc.receipt).toHaveBeenCalledTimes(writes.length);
        },
    );

    it.each(["local", "json-rpc"])(
        "uses the verified signer with a %s client",
        async (type) => {
            const local = privateKeyToAccount(`0x${"11".repeat(32)}`);
            const owner = type === "local" ? local.address : approval.owner;
            const wallet = createClient({
                chain: anvil,
                account:
                    type === "local" ? local : (approval.spender as Address),
                transport: custom({ request: vi.fn() }),
            });
            rpc.accounts.mockResolvedValue([owner]);
            await createWalletSession({
                publicClient: client,
                walletClient: wallet,
            }).sign({
                ...order,
                approval: { ...approval, owner },
            });
            expect(rpc.sign.mock.calls[0][1].account).toEqual(
                type === "local" ? local : owner,
            );
        },
    );

    it("waits for approval confirmation before signing", async () => {
        rpc.read.mockResolvedValueOnce(100n).mockResolvedValueOnce(0n);
        let confirm!: (value: { status: string }) => void;
        rpc.receipt.mockReturnValue(
            new Promise((resolve) => {
                confirm = resolve;
            }),
        );
        const pending = session.sign(order);
        await vi.waitFor(() => expect(rpc.receipt).toHaveBeenCalledOnce());
        expect(rpc.sign).not.toHaveBeenCalled();
        confirm({ status: "success" });
        await expect(pending).resolves.toBe("0xsigned");
    });

    it.each([
        "insufficient balance",
        "false approval",
        "reverted approval",
        "unchanged allowance",
        "switched account",
    ])("never signs after %s", async (failure) => {
        rpc.read
            .mockResolvedValueOnce(
                failure === "insufficient balance" ? 1n : 100n,
            )
            .mockResolvedValueOnce(0n);
        if (failure === "false approval")
            rpc.call.mockResolvedValue({ data: `0x${"0".repeat(64)}` });
        if (failure === "reverted approval")
            rpc.receipt.mockResolvedValue({ status: "reverted" });
        if (failure === "unchanged allowance") rpc.read.mockResolvedValue(0n);
        if (failure === "switched account") {
            rpc.accounts
                .mockResolvedValueOnce([approval.owner])
                .mockResolvedValue([approval.spender]);
        }
        await expect(session.sign(order)).rejects.toThrow();
        expect(rpc.sign).not.toHaveBeenCalled();
        if (
            failure === "insufficient balance" ||
            failure === "switched account"
        ) {
            expect(rpc.write).not.toHaveBeenCalled();
        }
    });
});

describe("transaction preflight", () => {
    const transaction = {
        owner: approval.owner,
        chainId: approval.chainId,
        to: approval.spender,
        data: "0x1234" as const,
        value: 0n,
    };

    it("simulates the exact transaction before broadcasting", async () => {
        await expect(session.sendTransaction(transaction)).resolves.toBe("0xsent");
        expect(rpc.call).toHaveBeenCalledWith(client, {
            account: transaction.owner,
            to: transaction.to,
            data: transaction.data,
            value: transaction.value,
        });
        expect(rpc.send).toHaveBeenCalledOnce();
    });

    it("does not broadcast when simulation reverts", async () => {
        rpc.call.mockRejectedValueOnce(new Error("execution reverted"));
        await expect(session.sendTransaction(transaction)).rejects.toThrow(
            "execution reverted",
        );
        expect(rpc.send).not.toHaveBeenCalled();
    });
});
