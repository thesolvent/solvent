import {
    erc20Abi,
    isAddressEqual,
    encodeFunctionData,
    decodeFunctionResult,
    type Account,
    type Address,
    type Client,
    type Chain,
    type Transport,
} from "viem";
import {
    getAddresses,
    getBlockNumber,
    getChainId,
    readContract,
    call,
    waitForTransactionReceipt,
    writeContract,
    signTypedData,
} from "viem/actions";

import type { OrderApproval, UnsignedOrder } from "../orders";

export interface WalletClients {
    publicClient: Client;
    walletClient: Client<Transport, Chain>;
}

export type TokenAccountRequest = Pick<
    OrderApproval,
    "token" | "owner" | "spender"
>;
export interface TokenAccount {
    balance: bigint;
    allowance: bigint;
}

/** Internal wallet adapter; clients are bound once and never owned by the SDK. */
export function createWalletSession({
    publicClient,
    walletClient,
}: WalletClients) {
    /** Recheck the live wallet after prompts: its account or network may have changed. */
    async function account({
        owner,
        chainId,
    }: Pick<OrderApproval, "owner" | "chainId">): Promise<Account | Address> {
        const [accounts, connectedChain] = await Promise.all([
            getAddresses(walletClient),
            getChainId(walletClient),
        ]);
        if (!accounts[0] || !isAddressEqual(accounts[0], owner)) {
            throw new Error("Wallet account changed");
        }
        if (connectedChain !== chainId) throw new Error("Wrong wallet network");
        // A connector's hoisted JSON-RPC account may lag behind its live eth_accounts response.
        return walletClient.account?.type === "local"
            ? walletClient.account
            : owner;
    }

    /** Read both prerequisites from the same block so approval decisions use a consistent view. */
    async function tokenAccount({
        token,
        owner,
        spender,
    }: TokenAccountRequest): Promise<TokenAccount> {
        const blockNumber = await getBlockNumber(publicClient, {
            cacheTime: 0,
        });
        const [balance, allowance] = await Promise.all([
            readContract(publicClient, {
                address: token,
                abi: erc20Abi,
                functionName: "balanceOf",
                args: [owner],
                blockNumber,
            }),
            readContract(publicClient, {
                address: token,
                abi: erc20Abi,
                functionName: "allowance",
                args: [owner, spender],
                blockNumber,
            }),
        ]);
        return { balance, allowance };
    }

    /** Cover an input with an exact allowance; reuse existing approval and await successful receipts. */
    async function ensureAllowance(approval: OrderApproval): Promise<void> {
        const { amount, chainId } = approval;
        if (amount <= 0n) throw new Error("Token amount must be positive");
        if ((await getChainId(publicClient)) !== chainId)
            throw new Error("Wrong RPC network");
        await account(approval);
        const { balance, allowance } = await tokenAccount(approval);
        if (balance < amount) throw new Error("Insufficient token balance");
        if (allowance >= amount) return;

        // Reset a partial allowance first, including tokens that reject nonzero-to-nonzero approvals.
        const amounts = allowance === 0n ? [amount] : [0n, amount];
        for (const value of amounts) await approve(approval, value);
    }

    async function approve(
        approval: OrderApproval,
        value: bigint,
    ): Promise<void> {
        const { owner, token, spender } = approval;
        const signer = await account(approval);
        const request = {
            account: owner,
            address: token,
            abi: erc20Abi,
            functionName: "approve",
            args: [spender, value],
        } as const;
        // ERC-20 permits no return value in practice; an explicit false still rejects approval.
        const { data } = await call(publicClient, {
            account: owner,
            to: token,
            data: encodeFunctionData(request),
        });
        if (
            data &&
            data !== "0x" &&
            !decodeFunctionResult({ ...request, data })
        ) {
            throw new Error("Token refused approval");
        }
        const hash = await writeContract(walletClient, {
            ...request,
            account: signer,
            chain: walletClient.chain,
        });
        const receipt = await waitForTransactionReceipt(publicClient, { hash });
        if (receipt.status !== "success") throw new Error("Approval reverted");
        // A successful receipt can be a cancellation or an approve that returned false.
        const current = await tokenAccount(approval);
        if (
            value === 0n ? current.allowance !== 0n : current.allowance < value
        ) {
            throw new Error("Token allowance was not updated");
        }
    }

    async function sign(order: UnsignedOrder) {
        assertNotExpired(order);
        await ensureAllowance(order.approval);
        const signer = await account(order.approval);
        assertNotExpired(order);
        return signTypedData(walletClient, {
            ...order.permit,
            account: signer,
        });
    }

    return { tokenAccount, sign };
}

function assertNotExpired(order: UnsignedOrder): void {
    if (order.deadline <= Math.floor(Date.now() / 1000)) {
        throw new Error("Swap order expired");
    }
}
