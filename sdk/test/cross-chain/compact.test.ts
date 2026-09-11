import { createClient, custom } from "viem";
import { anvil } from "viem/chains";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { depositCompact } from "../../src/cross-chain";

const rpc = vi.hoisted(() => ({
    accounts: vi.fn(),
    chain: vi.fn(),
    read: vi.fn(),
    write: vi.fn(),
    receipt: vi.fn(),
}));

vi.mock("viem/actions", async (original) => ({
    ...(await original<typeof import("viem/actions")>()),
    getAddresses: rpc.accounts,
    getChainId: rpc.chain,
    readContract: rpc.read,
    writeContract: rpc.write,
    waitForTransactionReceipt: rpc.receipt,
}));

const client = createClient({
    chain: anvil,
    transport: custom({ request: vi.fn() }),
});
const sponsor = "0x2222222222222222222222222222222222222222";

beforeEach(() => {
    vi.resetAllMocks();
    rpc.accounts.mockResolvedValue([
        "0x1111111111111111111111111111111111111111",
        sponsor,
    ]);
    rpc.chain.mockResolvedValue(anvil.id);
    rpc.read.mockResolvedValue(10n);
    rpc.write.mockResolvedValue("0xdeposit");
    rpc.receipt.mockResolvedValue({ status: "success" });
});

describe("Compact deposits", () => {
    it("uses the selected sponsor when the wallet exposes multiple accounts", async () => {
        await expect(
            depositCompact(client, client, {
                compact: "0x3333333333333333333333333333333333333333",
                token: "0x4444444444444444444444444444444444444444",
                lockTag: "0x0102030405060708090a0b0c",
                amount: 10n,
                sponsor,
            }),
        ).resolves.toBe("0xdeposit");

        expect(rpc.write).toHaveBeenCalledWith(
            client,
            expect.objectContaining({
                account: sponsor,
                functionName: "depositERC20",
                args: [
                    "0x4444444444444444444444444444444444444444",
                    "0x0102030405060708090a0b0c",
                    10n,
                    sponsor,
                ],
            }),
        );
    });
});
