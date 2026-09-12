import { defineChain, parseAbi, type Hex } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import {
    createPublicClient,
    createWalletClient,
    http,
} from "../lib/node-runtime.ts";
import { infraManifestPath, tryRead, type CrossChainInfraManifest } from "../deploy/manifests.ts";

const account = privateKeyToAccount(
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
);
const abi = parseAbi([
    "event MessageQueued(bytes32 indexed messageId,uint64 indexed destinationSelector,address indexed sourceOutbox,address receiver,bytes payload)",
    "function deliverPayload(bytes32 messageId,uint64 sourceSelector,address sourceOutbox,address receiver,bytes payload)",
]);

function chain(id: number, rpc: string) {
    return defineChain({
        id,
        name: `SolventX ${id}`,
        nativeCurrency: { name: "Ether", symbol: "ETH", decimals: 18 },
        rpcUrls: { default: { http: [rpc] } },
    });
}

/** `RELAY_ORIGIN_ROUTER`/`RELAY_DESTINATION_ROUTER` let the relay run as a standalone deployment
 *  (its own container, no shared filesystem with whatever deployed the contracts) — falls back to
 *  the local manifest file when unset, for the same-host devnet flow. */
function router(side: "origin" | "destination"): `0x${string}` {
    const envKey = side === "origin" ? "RELAY_ORIGIN_ROUTER" : "RELAY_DESTINATION_ROUTER";
    const fromEnv = process.env[envKey];
    if (fromEnv) return fromEnv as `0x${string}`;
    const infra = tryRead<CrossChainInfraManifest>(infraManifestPath(side));
    if (!infra)
        throw new Error(
            `no crosschain-infra manifest for ${side} and ${envKey} is unset — deploy it first (the relay has nothing to watch)`,
        );
    return infra.router as `0x${string}`;
}

function relay(
    sourceSide: "origin" | "destination",
    sourceId: number,
    sourceRpc: string,
    destinationSide: "origin" | "destination",
    destinationId: number,
    destinationRpc: string,
    sourceSelector: bigint,
) {
    const sourceChain = chain(sourceId, sourceRpc);
    const destinationChain = chain(destinationId, destinationRpc);
    const sourceRouter = router(sourceSide);
    const destinationRouter = router(destinationSide);
    const source = createPublicClient({ chain: sourceChain, transport: http(sourceRpc) });
    const destination = createPublicClient({
        chain: destinationChain,
        transport: http(destinationRpc),
    });
    const wallet = createWalletClient({
        account,
        chain: destinationChain,
        transport: http(destinationRpc),
    });
    const delivered = new Set<Hex>();
    source.watchContractEvent({
        address: sourceRouter,
        abi,
        eventName: "MessageQueued",
        poll: true,
        pollingInterval: 250,
        onLogs(logs) {
            for (const log of logs) {
                const { messageId, sourceOutbox, receiver, payload } = log.args;
                if (!messageId || !sourceOutbox || !receiver || !payload || delivered.has(messageId)) continue;
                delivered.add(messageId);
                void wallet
                    .writeContract({
                        address: destinationRouter,
                        abi,
                        functionName: "deliverPayload",
                        args: [messageId, sourceSelector, sourceOutbox, receiver, payload],
                    })
                    .then((hash) => destination.waitForTransactionReceipt({ hash }))
                    .then(() => console.log(`relayed ${sourceId} -> ${destinationId}: ${messageId}`))
                    .catch((error: unknown) => {
                        delivered.delete(messageId);
                        console.error(error instanceof Error ? error.message : "proof relay failed");
                    });
            }
        },
    });
}

const ORIGIN_RPC = process.env.RELAY_ORIGIN_RPC ?? "http://127.0.0.1:9645";
const DESTINATION_RPC = process.env.RELAY_DESTINATION_RPC ?? "http://127.0.0.1:9646";
const ORIGIN_CHAIN_ID = Number(process.env.RELAY_ORIGIN_CHAIN_ID ?? 31337);
const DESTINATION_CHAIN_ID = Number(process.env.RELAY_DESTINATION_CHAIN_ID ?? 31338);

relay("origin", ORIGIN_CHAIN_ID, ORIGIN_RPC, "destination", DESTINATION_CHAIN_ID, DESTINATION_RPC, 11n);
relay("destination", DESTINATION_CHAIN_ID, DESTINATION_RPC, "origin", ORIGIN_CHAIN_ID, ORIGIN_RPC, 22n);
console.log("SolventX local proof relay listening");
await new Promise(() => {});
