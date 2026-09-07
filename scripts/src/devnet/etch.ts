/**
 * Place canonical infra on the devnet chain.
 *
 * Multicall3 lives at one address on every real chain, and alloy's multicall builder reads that
 * address — but a fresh anvil hosts no code there, so every batched balance/allowance read fails to
 * decode. Foundry cheatcodes only mutate a script's local simulation, so the placement happens here
 * over `anvil_setCode` with the runtime bytecode our own build produced.
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { REPO_ROOT } from "../lib/manifest.ts";

const RPC_URL = process.env.SOLVENT_RPC_URL ?? "http://127.0.0.1:8545";

const MULTICALL3 = "0xcA11bde05977b3631167028862bE2a173976CA11";
const ARTIFACT = resolve(REPO_ROOT, "contracts/out/DevMulticall3.sol/DevMulticall3.json");

async function rpc<T>(method: string, params: unknown[]): Promise<T> {
  const response = await fetch(RPC_URL, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
  });
  const body = (await response.json()) as { result?: T; error?: { message: string } };
  if (body.error) throw new Error(`${method}: ${body.error.message}`);
  return body.result as T;
}

function runtimeBytecode(): string {
  try {
    const artifact = JSON.parse(readFileSync(ARTIFACT, "utf8")) as {
      deployedBytecode: { object: string };
    };
    return artifact.deployedBytecode.object;
  } catch (cause) {
    throw new Error(`no DevMulticall3 artifact — run 'forge build' in contracts/ first`, { cause });
  }
}

async function main(): Promise<void> {
  const code = runtimeBytecode();
  await rpc("anvil_setCode", [MULTICALL3, code]);

  const placed = await rpc<string>("eth_getCode", [MULTICALL3, "latest"]);
  if (placed.length <= 2) throw new Error(`multicall3 code did not stick at ${MULTICALL3}`);
  console.log(`etch: multicall3 ${MULTICALL3} <- ${(placed.length - 2) / 2} bytes`);
}

await main();
