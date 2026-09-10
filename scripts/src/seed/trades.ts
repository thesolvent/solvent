import { mkdir, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import {
    type Asset,
    type SolventClient,
    type Trade,
    SolventApiError,
    SolventNetworkError,
} from "@solvent/sdk/client";
import type { SwapClient, SwapIntent } from "@solvent/sdk/swap";
import {
    formatUnits,
    parseEther,
    parseUnits,
    type Address,
    type Hex,
} from "viem";
import { mnemonicToAccount } from "viem/accounts";
import { connectDevnet, failureMessage, type Devnet } from "../lib/devnet.ts";
import { REPO_ROOT } from "../lib/manifest.ts";
import { createSwapClient } from "../lib/node-runtime.ts";
import { PAIRS } from "./pairs.ts";

const REPORT_PATH = resolve(
    REPO_ROOT,
    process.env.SOLVENT_TRADE_REPORT ?? "devnet/generated/review-trades.json",
);
const TRADE_SIZES_USD = [1_000, 1_250, 1_500];
const TRADE_COUNT = PAIRS.length * TRADE_SIZES_USD.length;
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

interface TradeBatch {
    chainId: number;
    wallet: Address;
    trades: TradeResult[];
}

interface TradeRun {
    env: Devnet;
    swaps: SwapClient;
    batch: TradeBatch;
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

async function writeReport(batch: TradeBatch): Promise<void> {
    await writeFile(
        REPORT_PATH,
        JSON.stringify(
            {
                chainId: batch.chainId,
                wallet: batch.wallet,
                generatedAt: new Date().toISOString(),
                confirmed: batch.trades.filter((trade) => trade.receiptVerified)
                    .length,
                trades: batch.trades,
            },
            null,
            2,
        ) + "\n",
    );
}

async function waitForSettlement(
    api: SolventClient,
    id: string,
): Promise<Trade> {
    const deadline = Date.now() + 90_000;
    while (Date.now() < deadline) {
        const trade = await api.tradeDetail(id);
        if (TERMINAL.has(trade.status)) return trade;
        await sleep(1_000);
    }
    throw new Error(
        `Trade ${id} did not reach a terminal state within 90 seconds`,
    );
}

async function waitForPools(api: SolventClient): Promise<void> {
    // Registry and ledger ingestion follow confirmed chain writes asynchronously.
    const deadline = Date.now() + 60_000;
    while (true) {
        const pools = (await api.pools()).items;
        const indexed = new Set(
            pools.map((pool) =>
                [pool.base.symbol, pool.quote.symbol].sort().join("/"),
            ),
        );
        if (
            PAIRS.every((pair) =>
                indexed.has([pair.base, pair.quote].sort().join("/")),
            )
        )
            return;
        if (Date.now() >= deadline)
            throw new Error(
                `Not all ${PAIRS.length} seeded pairs are indexed; run seed first`,
            );
        await sleep(1_000);
    }
}

async function fundTaker(
    { api, publicClient, testClient }: Devnet,
    wallet: ReturnType<Devnet["wallet"]>,
): Promise<void> {
    const { address } = wallet.client.account;
    if ((await publicClient.getBalance({ address })) < parseEther("1"))
        await testClient.setBalance({ address, value: parseEther("10") });

    const assets = (await api.assets()).items;
    const symbols = new Set(PAIRS.flatMap((pair) => [pair.base, pair.quote]));
    for (const symbol of symbols) {
        const token = assets.find((asset) => asset.symbol === symbol);
        if (!token) throw new Error(`Missing asset ${symbol}`);
        await wallet.mint(token.address as Address, sizedInput(token, 20_000));
    }
}

async function executeTrade(
    { env, swaps, batch }: TradeRun,
    from: string,
    to: string,
    dollars: number,
): Promise<void> {
    const { api, config } = env;
    const current = (await api.assets()).items;
    const input = current.find((asset) => asset.symbol === from);
    const output = current.find((asset) => asset.symbol === to);
    if (!input || !output) throw new Error(`Missing assets for ${from}/${to}`);

    const amount = sizedInput(input, dollars);
    const quote = await api.quote({
        token_in: input.address,
        token_out: output.address,
        amount_in: amount.toString(),
    });
    const submitted = await submit(
        swaps.createIntent({
            swapper: batch.wallet,
            tokenIn: input.address,
            tokenOut: output.address,
            amountIn: amount,
            minAmountOut: (BigInt(quote.amount_out.raw) * 9_950n) / 10_000n,
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
    batch.trades.push(result);
    await writeReport(batch);

    const trade = await waitForSettlement(api, submitted.trade_id);
    result.status = trade.status;
    result.output = trade.output.amount.display;
    result.transaction = trade.tx_hash ?? null;
    result.explorerUrl = trade.tx_hash
        ? `${config.block_explorer_url.replace(/\/$/, "")}/tx/${trade.tx_hash}`
        : null;
    await writeReport(batch);
    if (trade.status !== "confirmed" || !trade.tx_hash)
        throw new Error(`Trade ${trade.id} ended ${trade.status}`);

    await env.confirm(trade.tx_hash as Hex);
    result.receiptVerified = true;
    await writeReport(batch);
    console.log(
        `  ${batch.trades.length}/${TRADE_COUNT}  ${from} → ${to}  ${trade.input.amount.display} → ${trade.output.amount.display}  ${trade.id} confirmed`,
    );
}

async function seedTrades(run: TradeRun): Promise<void> {
    try {
        for (const pair of PAIRS) {
            for (const [round, dollars] of TRADE_SIZES_USD.entries()) {
                const [from, to] =
                    round % 2 === 0
                        ? [pair.base, pair.quote]
                        : [pair.quote, pair.base];
                await executeTrade(run, from, to, dollars);
            }
        }
    } finally {
        await writeReport(run.batch);
    }
}

async function main(): Promise<void> {
    const env = await connectDevnet();
    // Public Anvil account #8 keeps sample taker trades separate from seed makers.
    const account = mnemonicToAccount(
        "test test test test test test test test test test test junk",
        { addressIndex: 8 },
    );
    const wallet = env.wallet(account);
    const swaps = createSwapClient({
        api: env.api,
        publicClient: env.publicClient,
        walletClient: wallet.client,
    });
    const batch: TradeBatch = {
        chainId: env.config.chain_id,
        wallet: account.address,
        trades: [],
    };

    await mkdir(dirname(REPORT_PATH), { recursive: true });
    await waitForPools(env.api);
    await fundTaker(env, wallet);
    console.log(
        `trades: ${PAIRS.length} pairs × ${TRADE_SIZES_USD.length} trades · taker ${account.address}`,
    );
    await seedTrades({ env, swaps, batch });
    console.log(
        `\n${batch.trades.length} confirmed trades across ${PAIRS.length} pools; report: ${REPORT_PATH}`,
    );
}

if (import.meta.main) {
    await main().catch((error) => {
        console.error(failureMessage(error));
        process.exitCode = 1;
    });
}
