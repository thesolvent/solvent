/**
 * Seed maker liquidity: mint, approve Aqua, then ship one strategy per pair.
 *
 * Programs are built with the same `@solvent/sdk` construction and positions modules a maker client
 * uses, so this exercises the real write path rather than a test-only shortcut.
 */
import { Strategy, linearWidthFromSymmetricRangePercent, positions } from "@solvent/sdk";
import { createSolventClient } from "@solvent/sdk/client";
import {
  createPublicClient,
  createWalletClient,
  defineChain,
  http,
  parseUnits,
  type Account,
  type Address,
  type Hex,
} from "viem";
import { privateKeyToAccount } from "viem/accounts";

import { readManifest, type Manifest } from "../lib/manifest.ts";

const RPC_URL = process.env.SOLVENT_RPC_URL ?? "http://127.0.0.1:8545";
const API_URL = process.env.SOLVENT_API_URL ?? "http://127.0.0.1:8080";

const api = createSolventClient({ baseUrl: API_URL });

// Anvil dev accounts #2.. — #0 deploys and owns the filler, #1 cosigns, so makers start at #2.
const MAKER_KEYS: readonly Hex[] = [
  "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
  "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
  "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a",
  "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba",
];

/** Minted per token, well above what is shipped, so a maker keeps a wallet balance behind it. */
const MINT_UNITS = 1_000_000;

interface PairSpec {
  base: string;
  quote: string;
  /** A stable pair prices off its own reserves rather than a mid. */
  pegged?: boolean;
  /** Half-width of the quoted range, in percent. */
  widthPct: number;
  feeBps: number;
  /** Base-token size shipped; the quote leg is sized at the mid. */
  size: number;
}

const PAIRS: readonly PairSpec[] = [
  { base: "WETH", quote: "USDC", widthPct: 8, feeBps: 5, size: 30 },
  { base: "WBTC", quote: "USDC", widthPct: 10, feeBps: 30, size: 2 },
  { base: "LINK", quote: "USDC", widthPct: 12, feeBps: 30, size: 20_000 },
  { base: "DAI", quote: "USDC", pegged: true, widthPct: 1, feeBps: 1, size: 250_000 },
];

/** How a pair is priced: a stable pair off its own reserves, anything else off a live mid. */
type Pricing = { kind: "pegged" } | { kind: "ranged"; mid: number };

const pairKey = (spec: PairSpec) => `${spec.base}/${spec.quote}`;

/** Mids come from the server's own oracle, so seeded pools quote near the market rather than a
 *  number that was current when this file was written. Rounded so a small move does not change
 *  the program — and with it the strategy hash — on every run. */
async function midPrices(): Promise<Map<string, number>> {
  const assets = await api.assets();
  const usd = new Map(
    assets.items.flatMap((asset) =>
      asset.price_usd == null ? [] : [[asset.symbol, asset.price_usd] as const],
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

/** A ranged pair with no mid is a dead feed, not a peg — say so rather than seeding it flat. */
function pricingFor(spec: PairSpec, mids: Map<string, number>): Pricing {
  if (spec.pegged) return { kind: "pegged" };
  const mid = mids.get(pairKey(spec));
  if (mid === undefined) {
    throw new Error(`no USD price for ${spec.base}/${spec.quote}; is the price feed up?`);
  }
  return { kind: "ranged", mid };
}

const devnet = defineChain({
  id: 31337,
  name: "Solvent Devnet",
  nativeCurrency: { name: "Ether", symbol: "ETH", decimals: 18 },
  rpcUrls: { default: { http: [RPC_URL] } },
});

const DEV_TOKEN_ABI = [
  {
    type: "function",
    name: "mint",
    stateMutability: "nonpayable",
    inputs: [
      { name: "to", type: "address" },
      { name: "amount", type: "uint256" },
    ],
    outputs: [],
  },
] as const;

// Aqua rejects re-shipping a strategy (`StrategiesMustBeImmutable`), so a repeat run checks whether
// this maker already holds a balance under the hash and skips instead of reverting.
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

const publicClient = createPublicClient({ chain: devnet, transport: http(RPC_URL) });

function token(manifest: Manifest, symbol: string): TokenRef {
  const found = manifest.tokens[symbol];
  if (!found) throw new Error(`manifest has no ${symbol}`);
  return { address: found.address as Address, decimals: found.decimals };
}

/** Size both legs: the quote leg mirrors the base leg's value at the mid. */
function legs(manifest: Manifest, spec: PairSpec, pricing: Pricing): Legs {
  const base = token(manifest, spec.base);
  const quote = token(manifest, spec.quote);
  const quoteSize = pricing.kind === "pegged" ? spec.size : spec.size * pricing.mid;
  return {
    base,
    quote,
    baseAmount: parseUnits(String(spec.size), base.decimals),
    quoteAmount: parseUnits(String(quoteSize), quote.decimals),
  };
}

/** A pegged curve prices off the shipped reserves; a ranged one off an external mid. */
function strategyFor(spec: PairSpec, sized: Legs, pricing: Pricing): Strategy {
  const curve =
    pricing.kind === "pegged"
      ? Strategy.pegged({
          tokenA: { ...sized.base, reserve: sized.baseAmount },
          tokenB: { ...sized.quote, reserve: sized.quoteAmount },
          linearWidth: linearWidthFromSymmetricRangePercent(spec.widthPct),
        })
      : Strategy.inRange({
          base: sized.base,
          quote: sized.quote,
          mid: String(pricing.mid),
          halfWidthPct: spec.widthPct,
        });
  return curve.fee(spec.feeBps);
}

async function isShipped(
  manifest: Manifest,
  maker: Address,
  strategyHash: Hex,
  token: Address,
): Promise<boolean> {
  const [, tokensCount] = await publicClient.readContract({
    address: manifest.aqua as Address,
    abi: AQUA_ABI,
    functionName: "rawBalances",
    args: [maker, manifest.router as Address, strategyHash, token],
  });
  return tokensCount > 0;
}

/** Bind a submitter to one maker: send, wait, and fail loudly on a revert. */
function submitter(account: Account) {
  const wallet = createWalletClient({ account, chain: devnet, transport: http(RPC_URL) });

  async function confirm(hash: Hex): Promise<void> {
    const receipt = await publicClient.waitForTransactionReceipt({ hash });
    if (receipt.status !== "success") throw new Error(`transaction reverted: ${hash}`);
  }

  return {
    async send(tx: { to: Address; data: Hex; value: bigint }): Promise<void> {
      await confirm(await wallet.sendTransaction({ ...tx, account, chain: devnet }));
    },
    async mint(leg: TokenRef): Promise<void> {
      await confirm(
        await wallet.writeContract({
          address: leg.address,
          abi: DEV_TOKEN_ABI,
          functionName: "mint",
          args: [account.address, parseUnits(String(MINT_UNITS), leg.decimals)],
          account,
          chain: devnet,
        }),
      );
    },
  };
}

async function seedPair(
  manifest: Manifest,
  spec: PairSpec,
  key: Hex,
  pricing: Pricing,
): Promise<string> {
  const account = privateKeyToAccount(key);
  const pos = positions({ aqua: manifest.aqua as Address, app: manifest.router as Address });
  const sized = legs(manifest, spec, pricing);
  const built = strategyFor(spec, sized, pricing).build(account.address);

  const at =
    pricing.kind === "pegged" ? "pegged" : `mid ${pricing.mid.toLocaleString("en-US")}`;
  const label = `${spec.base}/${spec.quote}  ${at.padEnd(14)} maker ${account.address.slice(0, 10)}`;
  if (await isShipped(manifest, account.address, built.strategyHash, sized.base.address)) {
    return `${label}  (already shipped)`;
  }

  const maker = submitter(account);
  for (const leg of [sized.base, sized.quote]) {
    await maker.mint(leg);
    await maker.send(pos.approve({ token: leg.address, amount: 2n ** 255n }));
  }

  await maker.send(
    pos.ship({
      strategy: built.order,
      amounts: [
        { token: sized.base.address, amount: sized.baseAmount },
        { token: sized.quote.address, amount: sized.quoteAmount },
      ],
    }),
  );

  return label;
}

async function main(): Promise<void> {
  const manifest = readManifest();
  const mids = await midPrices();
  console.log(`seed: ${RPC_URL} · aqua ${manifest.aqua}\n`);

  for (const [index, spec] of PAIRS.entries()) {
    const key = MAKER_KEYS[index % MAKER_KEYS.length];
    try {
      const pricing = pricingFor(spec, mids);
      console.log(`  ok    ${await seedPair(manifest, spec, key, pricing)}`);
    } catch (error) {
      console.log(`  FAIL  ${spec.base}/${spec.quote}: ${error instanceof Error ? error.message : error}`);
      process.exitCode = 1;
    }
  }
}

await main();
