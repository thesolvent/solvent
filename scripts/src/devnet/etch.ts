/** Install canonical infrastructure on the local devnet, matching the Rust integration harness. */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import {
    createTestClient,
    getTypesForEIP712Domain,
    hashDomain,
    http,
    parseAbi,
    publicActions,
    type Hex,
} from "viem";

import { REPO_ROOT } from "../lib/manifest.ts";

const RPC_URL = process.env.SOLVENT_RPC_URL ?? "http://127.0.0.1:8545";
const MULTICALL3 = "0xcA11bde05977b3631167028862bE2a173976CA11";
const PERMIT2 = "0x000000000022D473030F116dDEE9F6B43aC78BA3";
const ARTIFACT = resolve(
    REPO_ROOT,
    "contracts/out/DevMulticall3.sol/DevMulticall3.json",
);
const PERMIT2_RUNTIME = resolve(
    REPO_ROOT,
    "crates/adapters/tests/fixtures/permit2_runtime.hex",
);

function runtimeBytecode(): Hex {
    const artifact = JSON.parse(readFileSync(ARTIFACT, "utf8")) as {
        deployedBytecode: { object: Hex };
    };
    return artifact.deployedBytecode.object;
}

async function main(): Promise<void> {
    const client = createTestClient({
        mode: "anvil",
        transport: http(RPC_URL),
    }).extend(publicActions);
    const chainId = await client.getChainId();
    const contracts = [
        {
            name: "multicall3",
            address: MULTICALL3,
            bytecode: runtimeBytecode(),
        },
        {
            name: "permit2",
            address: PERMIT2,
            bytecode: readFileSync(PERMIT2_RUNTIME, "utf8").trim() as Hex,
        },
    ] as const;
    for (const contract of contracts) {
        await client.setCode(contract);
        const placed = await client.getCode({ address: contract.address });
        if (placed !== contract.bytecode)
            throw new Error(`${contract.name} code did not stick`);
        console.log(
            `etch: ${contract.name} ${contract.address} <- ${(placed.length - 2) / 2} bytes`,
        );
    }
    // Copied runtime contains immutable domain caches; verify this chain uses the canonical address.
    const domain = await client.readContract({
        address: PERMIT2,
        abi: parseAbi(["function DOMAIN_SEPARATOR() view returns (bytes32)"]),
        functionName: "DOMAIN_SEPARATOR",
    });
    const expected = {
        name: "Permit2",
        chainId,
        verifyingContract: PERMIT2,
    } as const;
    if (
        domain !==
        hashDomain<Record<string, unknown>>({
            domain: expected,
            types: {
                EIP712Domain: getTypesForEIP712Domain({ domain: expected }),
            },
        })
    ) {
        throw new Error(
            "Permit2 domain does not match the devnet signing domain",
        );
    }
    console.log("etch: Permit2 signing domain verified");
}

await main();
