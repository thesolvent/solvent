import {
    erc20Abi,
    type Account,
    type Address,
    type Chain,
    type Client,
    type Hex,
    type Transport,
} from "viem";
import {
    getAddresses,
    getChainId,
    readContract,
    signTypedData,
    waitForTransactionReceipt,
    writeContract,
} from "viem/actions";

const compactAbi = [
    {
        type: "function",
        name: "balanceOf",
        stateMutability: "view",
        inputs: [
            { name: "owner", type: "address" },
            { name: "id", type: "uint256" },
        ],
        outputs: [{ type: "uint256" }],
    },
    {
        type: "function",
        name: "depositERC20",
        stateMutability: "nonpayable",
        inputs: [
            { name: "token", type: "address" },
            { name: "lockTag", type: "bytes12" },
            { name: "amount", type: "uint256" },
            { name: "recipient", type: "address" },
        ],
        outputs: [{ name: "id", type: "uint256" }],
    },
] as const;

export interface CompactMandate {
    orderId: Hex;
    destinationChainId: bigint;
    destinationSettler: Address;
    fillProofVerifier: Address;
    outputToken: Address;
    minimumOutputAmount: bigint;
    recipient: Address;
    fillDeadline: number;
    exclusiveFiller: Address;
    routeKind: number;
}

export interface CompactCommitment {
    arbiter: Address;
    sponsor: Address;
    nonce: bigint;
    expires: bigint;
    lockTag: Hex;
    token: Address;
    amount: bigint;
    mandate: CompactMandate;
}

/** Lets callers update their UI once the wallet has broadcast a Compact transaction. */
export interface CompactTransactionOptions {
    onBroadcast?(): void;
}

export async function compactBalance(
    client: Client,
    compact: Address,
    sponsor: Address,
    id: bigint,
): Promise<bigint> {
    return readContract(client, {
        address: compact,
        abi: compactAbi,
        functionName: "balanceOf",
        args: [sponsor, id],
    });
}

export async function depositCompact(
    publicClient: Client,
    walletClient: Client<Transport, Chain>,
    request: {
        compact: Address;
        token: Address;
        lockTag: Hex;
        amount: bigint;
        sponsor: Address;
    },
    options?: CompactTransactionOptions,
): Promise<Hex> {
    if (request.amount <= 0n)
        throw new Error("Compact deposit must be positive");
    const [accounts, walletChain, rpcChain] = await Promise.all([
        getAddresses(walletClient),
        getChainId(walletClient),
        getChainId(publicClient),
    ]);
    if (
        !accounts.some(
            (account) =>
                account.toLowerCase() === request.sponsor.toLowerCase(),
        )
    ) {
        throw new Error("Wallet account changed");
    }
    if (walletChain !== rpcChain)
        throw new Error("Wallet and RPC networks differ");
    const allowance = await readContract(publicClient, {
        address: request.token,
        abi: erc20Abi,
        functionName: "allowance",
        args: [request.sponsor, request.compact],
    });
    if (allowance < request.amount)
        throw new Error("Compact allowance is insufficient");
    const account: Account | Address =
        walletClient.account?.type === "local"
            ? walletClient.account
            : request.sponsor;
    const hash = await writeContract(walletClient, {
        account,
        chain: walletClient.chain,
        address: request.compact,
        abi: compactAbi,
        functionName: "depositERC20",
        args: [request.token, request.lockTag, request.amount, request.sponsor],
    });
    options?.onBroadcast?.();
    const receipt = await waitForTransactionReceipt(publicClient, { hash });
    if (receipt.status !== "success")
        throw new Error("Compact deposit reverted");
    return hash;
}

export async function approveCompact(
    publicClient: Client,
    walletClient: Client<Transport, Chain>,
    request: {
        compact: Address;
        token: Address;
        amount: bigint;
        sponsor: Address;
    },
    options?: CompactTransactionOptions,
): Promise<Hex | undefined> {
    const allowance = await readContract(publicClient, {
        address: request.token,
        abi: erc20Abi,
        functionName: "allowance",
        args: [request.sponsor, request.compact],
    });
    if (allowance >= request.amount) return undefined;
    const account: Account | Address =
        walletClient.account?.type === "local"
            ? walletClient.account
            : request.sponsor;
    const hash = await writeContract(walletClient, {
        account,
        chain: walletClient.chain,
        address: request.token,
        abi: erc20Abi,
        functionName: "approve",
        args: [request.compact, request.amount],
    });
    options?.onBroadcast?.();
    const receipt = await waitForTransactionReceipt(publicClient, { hash });
    if (receipt.status !== "success")
        throw new Error("Compact approval reverted");
    return hash;
}

export async function signCompactMandate(
    walletClient: Client<Transport, Chain>,
    compact: Address,
    chainId: number,
    commitment: CompactCommitment,
): Promise<Hex> {
    if (commitment.expires <= BigInt(Math.floor(Date.now() / 1_000))) {
        throw new Error("Compact commitment expired");
    }
    return signTypedData(walletClient, {
        account: commitment.sponsor,
        domain: {
            name: "The Compact",
            version: "1",
            chainId,
            verifyingContract: compact,
        },
        primaryType: "Compact",
        types: {
            Compact: [
                { name: "arbiter", type: "address" },
                { name: "sponsor", type: "address" },
                { name: "nonce", type: "uint256" },
                { name: "expires", type: "uint256" },
                { name: "lockTag", type: "bytes12" },
                { name: "token", type: "address" },
                { name: "amount", type: "uint256" },
                { name: "mandate", type: "Mandate" },
            ],
            Mandate: [
                { name: "orderId", type: "bytes32" },
                { name: "destinationChainId", type: "uint256" },
                { name: "destinationSettler", type: "address" },
                { name: "fillProofVerifier", type: "address" },
                { name: "outputToken", type: "address" },
                { name: "minimumOutputAmount", type: "uint256" },
                { name: "recipient", type: "address" },
                { name: "fillDeadline", type: "uint48" },
                { name: "exclusiveFiller", type: "address" },
                { name: "routeKind", type: "uint8" },
            ],
        },
        message: commitment,
    });
}
