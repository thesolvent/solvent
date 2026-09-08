import {
  createSolventClient,
  SolventApiError,
  SolventNetworkError,
} from "@solvent/sdk/client";
import { createRequire } from "node:module";
import type { Account, Address, Hex } from "viem";
import { anvil } from "viem/chains";
import { readManifest } from "./manifest.ts";

// SDK and client actions must share viem's error constructors for receipt retries to work.
const {
  createPublicClient,
  createTestClient,
  createWalletClient,
  http,
  isAddressEqual,
  parseAbi,
} = createRequire(import.meta.url)("viem") as typeof import("viem");

const MINT_ABI = parseAbi(["function mint(address to, uint256 amount)"]);

/** Refuse writes unless the API, RPC, and deployment manifest describe this devnet. */
export async function connectDevnet() {
  const manifest = readManifest();
  const rpcUrl = process.env.SOLVENT_RPC_URL ?? "http://127.0.0.1:8545";
  const api = createSolventClient({
    baseUrl: process.env.SOLVENT_API_URL ?? "http://127.0.0.1:8080",
  });
  const publicClient = createPublicClient({
    chain: anvil,
    transport: http(rpcUrl),
    pollingInterval: 1_000,
  });
  const [config, rpcChain, assets, code] = await Promise.all([
    api.config(),
    publicClient.getChainId(),
    api.assets(),
    publicClient.getCode({ address: manifest.reactor as Address }),
  ]);
  if (
    [manifest.chain_id, config.chain_id, rpcChain].some((id) => id !== anvil.id)
  )
    throw new Error("Seeding requires Solvent Devnet (chain 31337)");
  if (
    !code ||
    code === "0x" ||
    !isAddressEqual(config.reactor as Address, manifest.reactor as Address) ||
    !isAddressEqual(config.permit2 as Address, manifest.permit2 as Address)
  )
    throw new Error("API and RPC do not match the deployment manifest");
  for (const [symbol, token] of Object.entries(manifest.tokens)) {
    const asset = assets.items.find((asset) => asset.symbol === symbol);
    if (
      !asset ||
      asset.decimals !== token.decimals ||
      !isAddressEqual(asset.address as Address, token.address as Address)
    )
      throw new Error(
        `API token ${symbol} does not match the deployment manifest`,
      );
  }

  async function confirm(hash: Hex) {
    const receipt = await publicClient.waitForTransactionReceipt({
      hash,
      timeout: 60_000,
    });
    if (receipt.status !== "success")
      throw new Error(`Transaction reverted: ${hash}`);
    return receipt;
  }

  function wallet(account: Account) {
    const client = createWalletClient({
      account,
      chain: anvil,
      transport: http(rpcUrl),
    });
    return {
      client,
      async mint(token: Address, amount: bigint) {
        await confirm(
          await client.writeContract({
            address: token,
            abi: MINT_ABI,
            functionName: "mint",
            args: [account.address, amount],
          }),
        );
      },
      async send(tx: { to: Address; data: Hex; value: bigint }) {
        await confirm(await client.sendTransaction(tx));
      },
    };
  }

  return {
    api,
    config,
    manifest,
    publicClient,
    wallet,
    confirm,
    testClient: createTestClient({
      mode: "anvil",
      chain: anvil,
      transport: http(rpcUrl),
    }),
  };
}

/** Transport errors can carry signed requests; print only their safe summary. */
export function failureMessage(error: unknown): string {
  if (error instanceof SolventApiError)
    return `API rejected request (HTTP ${error.status})`;
  if (error instanceof SolventNetworkError) return "API transport failed";
  // ESM and CommonJS SDK consumers can load separate viem error constructors.
  if (
    error instanceof Error &&
    "shortMessage" in error &&
    typeof error.shortMessage === "string"
  )
    return error.shortMessage;
  return error instanceof Error ? error.message : "Unknown failure";
}
