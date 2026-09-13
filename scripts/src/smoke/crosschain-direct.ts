/** Exercise the one local direct maker lane end-to-end with a deterministic Anvil taker. */
import { randomBytes } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

import {
    approveCompact,
    compactBalance,
    createCrossChainClient,
    depositCompact,
    signCompactMandate,
} from "@solvent/sdk/cross-chain";
import {
    createPublicClient,
    createWalletClient,
    defineChain,
    http,
    type Address,
    type Hex,
} from "viem";
import { mnemonicToAccount } from "viem/accounts";

import { REPO_ROOT, readManifest } from "../lib/manifest.ts";
import { failureMessage } from "../lib/devnet.ts";

const TAKER = mnemonicToAccount(
    "test test test test test test test test test test test junk",
    { addressIndex: 8 },
);
const WBTC_AMOUNT = 100_000n;
const ORDER_TIMEOUT_MS = 45_000;

interface Manifest {
    chain_id: number;
    tokens: Record<string, { address: Address; decimals: number }>;
}

function nonce(): Hex {
    return `0x${randomBytes(32).toString("hex")}`;
}

function required(name: string): string {
    const value = process.env[name];
    if (!value) throw new Error(`${name} must be set`);
    return value;
}

async function main(): Promise<void> {
    const origin = readManifest();
    const destinationPath = resolve(
        REPO_ROOT,
        "contracts/deployments/crosschain/destination/devnet.json",
    );
    const destination = JSON.parse(await readFile(destinationPath, "utf8")) as Manifest;
    const rpcUrl = required("SOLVENT_RPC_URL");
    const coordinator = process.env.SOLVENT_COORDINATOR_URL ?? "http://127.0.0.1:8090";
    const chain = defineChain({
        id: origin.chain_id,
        name: "Solvent cross-chain origin",
        nativeCurrency: { name: "Ether", symbol: "ETH", decimals: 18 },
        rpcUrls: { default: { http: [rpcUrl] } },
    });
    const publicClient = createPublicClient({ chain, transport: http(rpcUrl) });
    const walletClient = createWalletClient({ account: TAKER, chain, transport: http(rpcUrl) });
    const api = createCrossChainClient({ baseUrl: coordinator });
    const now = Math.floor(Date.now() / 1_000);
    const quote = await api.quote({
        request_id: nonce(),
        origin_chain_id: origin.chain_id,
        destination_chain_id: destination.chain_id,
        origin_token_in: origin.tokens.WBTC.address as Address,
        origin_token_out: origin.tokens.WBTC.address as Address,
        destination_token_in: destination.tokens.WBTC.address,
        destination_token_out: destination.tokens.USDC.address,
        amount_in: `0x${WBTC_AMOUNT.toString(16)}`,
        destination_amount_in: `0x${WBTC_AMOUNT.toString(16)}`,
        deadline_unix: now + 300,
        route: "direct",
    });
    if (quote.destination.sources.length !== 1) {
        throw new Error("direct WBTC/USDC quote must use exactly one maker source");
    }
    const request = {
        quote,
        sponsor: TAKER.address,
        recipient: TAKER.address,
        order_nonce: nonce(),
        compact_nonce: nonce(),
        compact_expires_unix: quote.expires_at_unix + 300,
    };
    const draft = await api.draft(request);
    const amount = BigInt(draft.commitment.amount);
    const compactId = BigInt(draft.order.compact_id);
    const deposited = await compactBalance(publicClient, draft.compact, TAKER.address, compactId);
    if (deposited < amount) {
        const needed = amount - deposited;
        await approveCompact(publicClient, walletClient, {
            compact: draft.compact,
            token: draft.commitment.token,
            amount: needed,
            sponsor: TAKER.address,
        });
        await depositCompact(publicClient, walletClient, {
            compact: draft.compact,
            token: draft.commitment.token,
            lockTag: draft.commitment.lock_tag,
            amount: needed,
            sponsor: TAKER.address,
        });
    }
    const terms = draft.commitment;
    const signature = await signCompactMandate(walletClient, draft.compact, origin.chain_id, {
        arbiter: terms.arbiter,
        sponsor: terms.sponsor,
        nonce: BigInt(terms.nonce),
        expires: BigInt(terms.expires),
        lockTag: terms.lock_tag,
        token: terms.token,
        amount: BigInt(terms.amount),
        mandate: {
            orderId: terms.mandate.order_id,
            destinationChainId: BigInt(terms.mandate.destination_chain_id),
            destinationSettler: terms.mandate.destination_settler,
            fillProofVerifier: terms.mandate.fill_proof_verifier,
            outputToken: terms.mandate.output_token,
            minimumOutputAmount: BigInt(terms.mandate.minimum_output_amount),
            recipient: terms.mandate.recipient,
            fillDeadline: terms.mandate.fill_deadline,
            exclusiveFiller: terms.mandate.exclusive_filler,
            routeKind: terms.mandate.route_kind,
        },
    });
    const submitted = await api.submitDirect({ draft: request, sponsor_signature: signature });
    const order = await api.wait(submitted.order_id, {
        intervalMs: 250,
        signal: AbortSignal.timeout(ORDER_TIMEOUT_MS),
    });
    if (order.state !== "complete") {
        throw new Error(`direct WBTC/USDC order stopped at ${order.state}`);
    }
    console.log(`cross-chain direct: WBTC -> USDC complete (${order.order_id})`);
}

if (import.meta.main) {
    await main().catch((error) => {
        console.error(failureMessage(error));
        process.exitCode = 1;
    });
}
