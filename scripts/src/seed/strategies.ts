/** Seed maker liquidity through the SDK's strategy construction and position write path. */
import type { SolventClient } from "@solvent/sdk/client";
import type {
    BuiltStrategy,
    Strategy as StrategyBuilder,
} from "@solvent/sdk/construction";
import { parseUnits, type Address, type Hex } from "viem";
import { privateKeyToAccount } from "viem/accounts";

import { type Manifest } from "../lib/manifest.ts";
import { connectDevnet, failureMessage, type Devnet } from "../lib/devnet.ts";
import {
    Strategy,
    linearWidthFromSymmetricRangePercent,
    positions,
} from "../lib/node-runtime.ts";
import { PAIRS, type PairSpec } from "./pairs.ts";

// Anvil dev accounts #2.. — #0 deploys and owns the filler, #1 cosigns, so makers start at #2.
const MAKER_KEYS: readonly Hex[] = [
    "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
    "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
    "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a",
    "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba",
];

/** Minted per token, well above what is shipped, so a maker keeps a wallet balance behind it. */
const MINT_UNITS = 1_000_000;
const COPIES_PER_PAIR = 2;

type Pricing = { kind: "pegged" } | { kind: "ranged"; mid: number };

const pairKey = (spec: PairSpec) => `${spec.base}/${spec.quote}`;

/** Round oracle mids so small market moves do not change the program's strategy hash. */
async function midPrices(api: SolventClient): Promise<Map<string, number>> {
    const assets = await api.assets();
    const usd = new Map(
        assets.items.flatMap((asset) =>
            asset.price_usd == null
                ? []
                : [[asset.symbol, asset.price_usd] as const],
        ),
    );

    const mids = new Map<string, number>();
    for (const spec of PAIRS) {
        const base = usd.get(spec.base);
        const quote = usd.get(spec.quote);
        if (spec.pegged || base === undefined || quote === undefined) continue;
        mids.set(pairKey(spec), Number((base / quote).toPrecision(4)));
    }
    return mids;
}

function pricingFor(spec: PairSpec, mids: Map<string, number>): Pricing {
    if (spec.pegged) return { kind: "pegged" };
    const mid = mids.get(pairKey(spec));
    if (mid === undefined) {
        throw new Error(
            `no USD price for ${spec.base}/${spec.quote}; is the price feed up?`,
        );
    }
    return { kind: "ranged", mid };
}

// Aqua retains a docked hash forever: tokensCount 0 is unused, 255 is docked, and the rest are active.
const AQUA_ABI = [
    {
        type: "function",
        name: "rawBalances",
        stateMutability: "view",
        inputs: [
            { name: "maker", type: "address" },
            { name: "app", type: "address" },
            { name: "strategyHash", type: "bytes32" },
            { name: "token", type: "address" },
        ],
        outputs: [
            { name: "balance", type: "uint248" },
            { name: "tokensCount", type: "uint8" },
        ],
    },
] as const;

interface TokenRef {
    address: Address;
    decimals: number;
}

interface Legs {
    base: TokenRef;
    quote: TokenRef;
    baseAmount: bigint;
    quoteAmount: bigint;
}

function token(manifest: Manifest, symbol: string): TokenRef {
    const found = manifest.tokens[symbol];
    if (!found) throw new Error(`manifest has no ${symbol}`);
    return { address: found.address as Address, decimals: found.decimals };
}

function legs(manifest: Manifest, spec: PairSpec, pricing: Pricing): Legs {
    const base = token(manifest, spec.base);
    const quote = token(manifest, spec.quote);
    const quoteSize =
        pricing.kind === "pegged" ? spec.size : spec.size * pricing.mid;
    return {
        base,
        quote,
        baseAmount: parseUnits(String(spec.size), base.decimals),
        quoteAmount: parseUnits(String(quoteSize), quote.decimals),
    };
}

function strategyFor(
    spec: PairSpec,
    sized: Legs,
    pricing: Pricing,
): StrategyBuilder {
    const curve =
        pricing.kind === "pegged"
            ? Strategy.pegged({
                  tokenA: { ...sized.base, reserve: sized.baseAmount },
                  tokenB: { ...sized.quote, reserve: sized.quoteAmount },
                  linearWidth: linearWidthFromSymmetricRangePercent(
                      spec.widthPct,
                  ),
              })
            : Strategy.inRange({
                  base: sized.base,
                  quote: sized.quote,
                  mid: String(pricing.mid),
                  halfWidthPct: spec.widthPct,
              });
    return curve.fee(spec.feeBps);
}

async function tokensCount(
    { manifest, publicClient }: Devnet,
    maker: Address,
    strategyHash: Hex,
    token: Address,
): Promise<number> {
    const [, tokensCount] = await publicClient.readContract({
        address: manifest.aqua as Address,
        abi: AQUA_ABI,
        functionName: "rawBalances",
        args: [maker, manifest.router as Address, strategyHash, token],
    });
    return tokensCount;
}

export async function seedPair(
    env: Devnet,
    spec: PairSpec,
    key: Hex,
    pricing: Pricing,
): Promise<string> {
    const { config, manifest } = env;
    const account = privateKeyToAccount(key);
    const pos = positions({
        aqua: manifest.aqua as Address,
        app: manifest.router as Address,
    });
    const sized = legs(manifest, spec, pricing);
    const strategy = strategyFor(spec, sized, pricing);
    const pending: BuiltStrategy[] = [];
    let active = 0;
    for (let salt = 0n; active + pending.length < COPIES_PER_PAIR; salt += 1n) {
        const built = strategy
            .salt(salt)
            .build(account.address, config.taker_credential as Address);
        const count = await tokensCount(
            env,
            account.address,
            built.strategyHash,
            sized.base.address,
        );
        if (count === 0) pending.push(built);
        else if (count < 255) active += 1;
    }

    const at =
        pricing.kind === "pegged"
            ? "pegged"
            : `mid ${pricing.mid.toLocaleString("en-US")}`;
    const label = `${spec.base}/${spec.quote}  ${at.padEnd(14)} maker ${account.address.slice(0, 10)}`;
    if (pending.length === 0) {
        return `${label}  (${active} active copies; already shipped)`;
    }

    const maker = env.wallet(account);
    for (const leg of [sized.base, sized.quote]) {
        await maker.mint(
            leg.address,
            parseUnits(String(MINT_UNITS), leg.decimals),
        );
        await maker.send(
            pos.approve({ token: leg.address, amount: 2n ** 255n }),
        );
    }

    for (const built of pending) {
        await maker.send(
            pos.ship({
                strategy: built.order,
                amounts: [
                    { token: sized.base.address, amount: sized.baseAmount },
                    { token: sized.quote.address, amount: sized.quoteAmount },
                ],
            }),
        );
    }

    return `${label}  (${pending.length} shipped; ${COPIES_PER_PAIR} active copies)`;
}

async function main(): Promise<void> {
    const env = await connectDevnet();
    const { manifest } = env;
    const mids = await midPrices(env.api);
    console.log(
        `seed: ${PAIRS.length} pairs · ${COPIES_PER_PAIR} active copies each · aqua ${manifest.aqua}\n`,
    );

    for (const [index, spec] of PAIRS.entries()) {
        const key = MAKER_KEYS[index % MAKER_KEYS.length];
        try {
            const pricing = pricingFor(spec, mids);
            console.log(`  ok    ${await seedPair(env, spec, key, pricing)}`);
        } catch (error) {
            console.log(
                `  FAIL  ${spec.base}/${spec.quote}: ${failureMessage(error)}`,
            );
            process.exitCode = 1;
        }
    }
}

if (import.meta.main) {
    await main().catch((error) => {
        console.error(failureMessage(error));
        process.exitCode = 1;
    });
}
