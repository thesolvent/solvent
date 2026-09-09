import {
    decodeFunctionResult,
    encodeFunctionData,
    erc20Abi,
    isAddressEqual,
    type Account,
    type Address,
    type Client,
    type Chain,
    type Hex,
    type Transport,
} from "viem";
import {
    call,
    getAddresses,
    getBlockNumber,
    getChainId,
    readContract,
    sendTransaction as broadcastTransaction,
    signTypedData,
    waitForTransactionReceipt,
    writeContract,
} from "viem/actions";

import {
    assertFutureDeadline,
    type OrderApproval,
    type UnsignedOrder,
} from "../orders";

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

export interface AllowanceRequest extends OrderApproval {
    /** Allow a reusable approval while checking only the amount this action needs. */
    approvalAmount?: bigint;
}

export interface WalletTransaction {
    owner: Address;
    chainId: number;
    to: Address;
    data: Hex;
    value: bigint;
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

    async function requireSuccessfulReceipt(
        hash: Hex,
        revertedMessage: string,
    ): Promise<void> {
        const receipt = await waitForTransactionReceipt(publicClient, { hash });
        if (receipt.status !== "success") throw new Error(revertedMessage);
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
    async function ensureAllowance(approval: AllowanceRequest): Promise<void> {
        const { amount, chainId } = approval;
        if (amount <= 0n) throw new Error("Token amount must be positive");
        if ((await getChainId(publicClient)) !== chainId)
            throw new Error("Wrong RPC network");
        await account(approval);
        const { balance, allowance } = await tokenAccount(approval);
        if (balance < amount) throw new Error("Insufficient token balance");
        if (allowance >= amount) return;

        // Reset a partial allowance first, including tokens that reject nonzero-to-nonzero approvals.
        const target = approval.approvalAmount ?? amount;
        if (target < amount)
            throw new Error("Approval cannot be below the required amount");
        const amounts = allowance === 0n ? [target] : [0n, target];
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
        await requireSuccessfulReceipt(hash, "Approval reverted");
        // A successful receipt can be a cancellation or an approve that returned false.
        const current = await tokenAccount(approval);
        if (
            value === 0n ? current.allowance !== 0n : current.allowance < value
        ) {
            throw new Error("Token allowance was not updated");
        }
    }

    async function sign(order: UnsignedOrder) {
        assertFutureDeadline(order.deadline);
        await ensureAllowance(order.approval);
        const signer = await account(order.approval);
        assertFutureDeadline(order.deadline);
        return signTypedData(walletClient, {
            ...order.permit,
            account: signer,
        });
    }

    async function sendTransaction(request: WalletTransaction): Promise<Hex> {
        if ((await getChainId(publicClient)) !== request.chainId) {
            throw new Error("Wrong RPC network");
        }
        const signer = await account(request);
        await call(publicClient, {
            account: request.owner,
            to: request.to,
            data: request.data,
            value: request.value,
        });
        return broadcastTransaction(walletClient, {
            account: signer,
            chain: walletClient.chain,
            to: request.to,
            data: request.data,
            value: request.value,
        });
    }

    async function confirmTransaction(hash: Hex): Promise<void> {
        await requireSuccessfulReceipt(hash, "Transaction reverted");
    }

    return {
        tokenAccount,
        ensureAllowance,
        sendTransaction,
        confirmTransaction,
        sign,
    };
}
