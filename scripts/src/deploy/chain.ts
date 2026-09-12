/** Thin per-chain RPC helpers the deploy orchestrator needs: waiting for a fresh anvil to accept
 *  connections, reading a deployer's nonce (to precompute a not-yet-deployed contract's address),
 *  and confirming a manifest's addresses actually hold code on *this* chain right now — the check
 *  that would have caught the stray-anvil bug immediately instead of an hour of confusion. */
import {
  getContractAddress,
  type Address,
  type PublicClient,
} from "viem";

import { createPublicClient, http } from "../lib/node-runtime.ts";

export function client(rpcUrl: string): PublicClient {
  return createPublicClient({ transport: http(rpcUrl) }) as PublicClient;
}

export async function waitForRpc(
  rpcUrl: string,
  expectedChainId: number,
  timeoutMs = 60_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let lastError: unknown;
  while (Date.now() < deadline) {
    try {
      const id = await client(rpcUrl).getChainId();
      if (id !== expectedChainId) {
        throw new Error(
          `${rpcUrl} answers as chain ${id}, expected ${expectedChainId} — ` +
            "wrong endpoint, or something else is bound to this port",
        );
      }
      return;
    } catch (error) {
      lastError = error;
      await new Promise((r) => setTimeout(r, 1_000));
    }
  }
  throw new Error(
    `${rpcUrl} never became reachable as chain ${expectedChainId} within ${timeoutMs}ms`,
    { cause: lastError },
  );
}

/** The address a contract WILL get if `deployer`'s very next transaction on this chain is a
 *  `CREATE`. Used to break the origin-settler/destination-app circular constructor dependency:
 *  compute both future addresses before deploying either side, deploy in either order, then
 *  verify each landed exactly where predicted. */
export async function nextCreateAddress(
  rpcUrl: string,
  deployer: Address,
): Promise<Address> {
  const nonce = await client(rpcUrl).getTransactionCount({ address: deployer });
  return getContractAddress({ from: deployer, nonce: BigInt(nonce) });
}

export async function hasCode(rpcUrl: string, address: string): Promise<boolean> {
  const code = await client(rpcUrl).getCode({ address: address as Address });
  return Boolean(code) && code !== "0x";
}

/** Every address in `addresses` must hold real code on `rpcUrl`. Throws naming the first mismatch
 *  rather than returning a bool — the caller always wants to know *which* address is wrong. */
export async function assertDeployed(
  rpcUrl: string,
  addresses: Record<string, string>,
): Promise<void> {
  for (const [label, address] of Object.entries(addresses)) {
    if (!(await hasCode(rpcUrl, address))) {
      throw new Error(
        `${label} (${address}) has no code on ${rpcUrl} — the manifest is stale ` +
          "(chain state does not match the recorded deploy; rerun with --reset)",
      );
    }
  }
}
