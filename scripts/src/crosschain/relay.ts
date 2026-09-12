import { defineChain, parseAbi, type Hex } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import {
    createPublicClient,
    createWalletClient,
    http,
} from "../lib/node-runtime.ts";

const router = "0x610178dA211FEF7D417bC0e6FeD39F05609AD788";
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

function relay(sourceId: number, sourceRpc: string, destinationId: number, destinationRpc: string, sourceSelector: bigint) {
    const sourceChain = chain(sourceId, sourceRpc);
    const destinationChain = chain(destinationId, destinationRpc);
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
        address: router,
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
                        address: router,
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

relay(31337, "http://127.0.0.1:8545", 31338, "http://127.0.0.1:8546", 11n);
relay(31338, "http://127.0.0.1:8546", 31337, "http://127.0.0.1:8545", 22n);
console.log("SolventX local proof relay listening");
await new Promise(() => {});
