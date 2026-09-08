import { mkdir, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import {
  type Asset,
  type Trade,
  SolventApiError,
  SolventNetworkError,
} from "@solvent/sdk/client";
import type { SwapIntent } from "@solvent/sdk/swap";
import {
  formatUnits,
  parseEther,
  parseUnits,
  type Address,
  type Hex,
} from "viem";
import { mnemonicToAccount } from "viem/accounts";
import { connectDevnet, failureMessage } from "../lib/devnet.ts";
import { REPO_ROOT } from "../lib/manifest.ts";
import { PAIRS } from "./pairs.ts";

// Upstream SDK ESM builds assume a bundler; Node uses the published CommonJS entry point.
const { createSwapClient } = createRequire(import.meta.url)(
  "@solvent/sdk/swap",
) as typeof import("@solvent/sdk/swap");

const REPORT_PATH = resolve(REPO_ROOT, "devnet/generated/review-trades.json");
const TRADE_SIZES_USD = [1_000, 1_250, 1_500];
const TERMINAL = new Set(["confirmed", "declined", "failed"]);

interface TradeResult {
  pair: string;
  input: string;
  output: string;
  tradeId: string;
  status: string;
  receiptVerified: boolean;
  transaction: string | null;
  explorerUrl: string | null;
}

function sizedInput(token: Asset, dollars: number): bigint {
  if (
    token.price_usd == null ||
    !Number.isFinite(token.price_usd) ||
    token.price_usd <= 0
  )
    throw new Error(`No current USD price for ${token.symbol}`);
  return parseUnits(
    (dollars / token.price_usd).toFixed(Math.min(token.decimals, 8)),
    token.decimals,
  );
}

async function submit(intent: SwapIntent) {
  try {
    return await intent.submit();
  } catch (error) {
    if (
      !(error instanceof SolventNetworkError) &&
      !(error instanceof SolventApiError && error.status >= 500)
    )
      throw error;
    // Preserve the same signed authorization if the server's response was lost.
    await sleep(1_000);
    return intent.submit();
  }
}

async function main() {
  const env = await connectDevnet();
  const { api, publicClient, config } = env;
  // Public Anvil account #8 is dedicated to sample taker trades, separate from seed makers.
  const account = mnemonicToAccount(
    "test test test test test test test test test test test junk",
    {
      addressIndex: 8,
    },
  );
  const wallet = env.wallet(account);
  const swaps = createSwapClient({
    api,
    publicClient,
    walletClient: wallet.client,
  });
  const results: TradeResult[] = [];
  await mkdir(resolve(REPO_ROOT, "devnet/generated"), { recursive: true });

  async function report() {
    await writeFile(
      REPORT_PATH,
      JSON.stringify(
        {
          chainId: config.chain_id,
          wallet: account.address,
          generatedAt: new Date().toISOString(),
          confirmed: results.filter((result) => result.receiptVerified).length,
          trades: results,
        },
        null,
        2,
      ) + "\n",
    );
  }

  async function settled(id: string): Promise<Trade> {
    const until = Date.now() + 90_000;
    while (Date.now() < until) {
      const trade = await api.tradeDetail(id);
      if (TERMINAL.has(trade.status)) return trade;
      await sleep(1_000);
    }
    throw new Error(
      `Trade ${id} did not reach a terminal state within 90 seconds`,
    );
  }

  // Registry and ledger ingestion follow confirmed chain writes asynchronously.
  const poolDeadline = Date.now() + 60_000;
  while (true) {
    const pools = (await api.pools()).items;
    const ready = PAIRS.every((pair) =>
      pools.some(
        (pool) =>
          [pool.base.symbol, pool.quote.symbol].sort().join("/") ===
          [pair.base, pair.quote].sort().join("/"),
      ),
    );
    if (ready) break;
    if (Date.now() >= poolDeadline)
      throw new Error("Not all eight seeded pairs are indexed; run seed first");
    await sleep(1_000);
  }

  if (
    (await publicClient.getBalance({ address: account.address })) <
    parseEther("1")
  )
    await env.testClient.setBalance({
      address: account.address,
      value: parseEther("10"),
    });
  const assets = (await api.assets()).items;
  const symbols = new Set(PAIRS.flatMap((pair) => [pair.base, pair.quote]));
  for (const symbol of symbols) {
    const token = assets.find((asset) => asset.symbol === symbol);
    if (!token) throw new Error(`Missing asset ${symbol}`);
    await wallet.mint(token.address as Address, sizedInput(token, 20_000));
  }

  console.log(
    `trades: ${PAIRS.length} pairs × ${TRADE_SIZES_USD.length} trades · taker ${account.address}`,
  );
  try {
    for (const pair of PAIRS) {
      for (const [round, dollars] of TRADE_SIZES_USD.entries()) {
        const [from, to] =
          round % 2 === 0 ? [pair.base, pair.quote] : [pair.quote, pair.base];
        const current = (await api.assets()).items;
        const input = current.find((asset) => asset.symbol === from);
        const output = current.find((asset) => asset.symbol === to);
        if (!input || !output)
          throw new Error(`Missing assets for ${from}/${to}`);
        const amount = sizedInput(input, dollars);
        const quote = await api.quote({
          token_in: input.address,
          token_out: output.address,
          amount_in: amount.toString(),
        });
        const minimum = (BigInt(quote.amount_out.raw) * 9_950n) / 10_000n;
        const submitted = await submit(
          swaps.createIntent({
            swapper: account.address,
            tokenIn: input.address,
            tokenOut: output.address,
            amountIn: amount,
            minAmountOut: minimum,
            deadline: Math.floor(Date.now() / 1_000) + 600,
          }),
        );
        const result: TradeResult = {
          pair: `${from}/${to}`,
          input: formatUnits(amount, input.decimals),
          output: "pending",
          tradeId: submitted.trade_id,
          status: submitted.status,
          receiptVerified: false,
          transaction: null,
          explorerUrl: null,
        };
        results.push(result);
        await report();
        const trade = await settled(submitted.trade_id);
        result.status = trade.status;
        result.output = trade.output.amount.display;
        result.transaction = trade.tx_hash ?? null;
        result.explorerUrl = trade.tx_hash
          ? `${config.block_explorer_url.replace(/\/$/, "")}/tx/${trade.tx_hash}`
          : null;
        await report();
        if (trade.status !== "confirmed" || !trade.tx_hash)
          throw new Error(`Trade ${trade.id} ended ${trade.status}`);
        await env.confirm(trade.tx_hash as Hex);
        result.receiptVerified = true;
        await report();
        console.log(
          `  ${results.length}/24  ${from} → ${to}  ${trade.input.amount.display} → ${trade.output.amount.display}  ${trade.id} confirmed`,
        );
      }
    }
  } finally {
    await report();
  }
  console.log(`\n24 confirmed trades across 8 pools; report: ${REPORT_PATH}`);
}

await main().catch((error) => {
  console.error(failureMessage(error));
  process.exitCode = 1;
});
