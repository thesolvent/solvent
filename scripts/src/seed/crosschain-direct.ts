/** Seed the one direct WBTC-origin / USDC-destination maker lane used by the local cross-chain stack. */
import { mkdir, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";

import { type Address, type Hex } from "viem";

import { REPO_ROOT } from "../lib/manifest.ts";
import { connectDevnet, failureMessage } from "../lib/devnet.ts";
import { MAKER_KEYS, seedPair } from "./strategies.ts";

const PAIR = { base: "WBTC", quote: "USDC", widthPct: 10, feeBps: 30, size: 100 };
const MINT_UNITS = 10_000_000;

type Side = "origin" | "destination";

function required(name: string): string {
    const value = process.env[name];
    if (!value) throw new Error(`${name} must be set`);
    return value;
}

async function main(): Promise<void> {
    const side = required("SOLVENT_DIRECT_SIDE") as Side;
    if (side !== "origin" && side !== "destination") {
        throw new Error("SOLVENT_DIRECT_SIDE must be origin or destination");
    }
    const app = required("SOLVENT_STRATEGY_APP") as Address;
    const output = resolve(REPO_ROOT, required("SOLVENT_DIRECT_OUTPUT"));
    const env = await connectDevnet();
    const assets = await env.api.assets();
    const wbtc = assets.items.find((asset) => asset.symbol === PAIR.base)?.price_usd;
    const usdc = assets.items.find((asset) => asset.symbol === PAIR.quote)?.price_usd;
    if (!wbtc || !usdc) throw new Error("WBTC/USDC market price is unavailable");

    const seeded = await seedPair(
        env,
        PAIR,
        MAKER_KEYS[0],
        { kind: "ranged", mid: Number((wbtc / usdc).toPrecision(4)) },
        { app, copies: 1, mintUnits: MINT_UNITS, configureDirect: side === "destination" },
    );
    const [strategyHash] = seeded.strategyHashes;
    if (!strategyHash) throw new Error("direct strategy was already seeded without a route record");

    await mkdir(dirname(output), { recursive: true });
    await writeFile(
        output,
        `${JSON.stringify({ side, maker: seeded.maker, strategyHash: strategyHash as Hex }, null, 2)}\n`,
    );
    console.log(`direct ${side}: ${seeded.summary}`);
}

if (import.meta.main) {
    await main().catch((error) => {
        console.error(failureMessage(error));
        process.exitCode = 1;
    });
}
