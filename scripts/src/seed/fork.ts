/** Seed maker liquidity on a mainnet fork, through the same SDK path a real maker uses.
 *
 * The devnet seeder mints DevTokens; on a fork the tokens are the real ones, so balances are set
 * directly in their storage and the maker then approves and ships exactly as it would on mainnet.
 * Prices are deliberately settable: shipping a strategy that sells its output below market is how
 * we test whether the resolver takes an order that is only profitable because the maker mispriced.
 */
import { parseUnits, type Address, type Hex } from "viem";
import { privateKeyToAccount } from "viem/accounts";

import {
    Strategy,
    positions,
    createPublicClient,
    createWalletClient,
    http,
} from "../lib/node-runtime.ts";

const RPC = process.env.FORK_RPC ?? "http://127.0.0.1:8546";
const AQUA = (process.env.AQUA ??
    "0x1111113ccf1426a8e30e2bff5e005d929bf6a90a") as Address;
const APP = (process.env.APP ??
    "0x111111338c5091e8440b67b168bae16a668ac0de") as Address;
const MAKER_KEY = (process.env.MAKER_KEY ??
    "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a") as Hex;

/** Mainnet ERC-20s and the storage slot holding their `balanceOf` mapping. */
const TOKENS = {
    WETH: { address: "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2", decimals: 18, slot: 3 },
    USDC: { address: "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", decimals: 6, slot: 9 },
    USDT: { address: "0xdac17f958d2ee523a2206206994597c13d831ec7", decimals: 6, slot: 2 },
    DAI:  { address: "0x6b175474e89094c44da98b954eedeac495271d0f", decimals: 18, slot: 2 },
    WBTC: { address: "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599", decimals: 8, slot: 0 },
    LINK: { address: "0x514910771af9ca656af840dff83e8264ecf986ca", decimals: 18, slot: 1 },
    UNI:  { address: "0x1f9840a85d5af5bf1d1762f925bdaddc4201f984", decimals: 18, slot: 4 },
} as const;
type Sym = keyof typeof TOKENS;

/** One position: the pair, how much of each side to ship, and the price band to quote in. */
interface Position {
    base: Sym;
    quote: Sym;
    baseAmount: string;
    quoteAmount: string;
    /** Quote per 1 base. Set below market to ship a position that loses money. */
    priceMin: string;
    priceMax: string;
    feeBps: number;
}

const POSITIONS: Position[] = JSON.parse(
    process.env.POSITIONS ??
        JSON.stringify([
            { base: "WETH", quote: "USDC", baseAmount: "20", quoteAmount: "80000", priceMin: "1500", priceMax: "2500", feeBps: 1 },
        ]),
);

/** Dock (close) the strategies listed in DOCK_FILE, so stale positions stop being routable. */
async function dockAll(
    send: (tx: { to: Address; data: Hex; value: bigint }) => Promise<Hex>,
    pos: ReturnType<typeof positions>,
    maker: Address,
): Promise<void> {
    const file = process.env.DOCK_FILE;
    if (!file) return;
    const list = JSON.parse(
        await (await import("node:fs/promises")).readFile(file, "utf8"),
    ) as { strategyHash: Hex; tokens: Address[] }[];
    let done = 0;
    for (const s of list) {
        try {
            await send(pos.dock({ strategyHash: s.strategyHash, tokens: s.tokens }));
            done += 1;
        } catch {
            // Not this maker's strategy, or already docked — both are fine to skip.
        }
    }
    console.log(`docked ${done}/${list.length} for ${maker}`);
}

async function main(): Promise<void> {
    const account = privateKeyToAccount(MAKER_KEY);
    const chain = undefined;
    const pub = createPublicClient({ transport: http(RPC), chain });
    const wallet = createWalletClient({ account, transport: http(RPC), chain });
    const pos = positions({ aqua: AQUA, app: APP });

    const send = async (tx: { to: Address; data: Hex; value: bigint }) => {
        const hash = await wallet.sendTransaction({ ...tx, account, chain });
        const receipt = await pub.waitForTransactionReceipt({ hash });
        if (receipt.status !== "success") throw new Error(`reverted: ${hash}`);
        return hash;
    };

    await dockAll(send, pos, account.address);

    // Balances first: on a fork the maker holds real tokens, so write them into the token's own
    // storage rather than minting. This is the only step a mainnet maker would not perform.
    const funded = new Set<Sym>();
    for (const p of POSITIONS) {
        for (const sym of [p.base, p.quote] as Sym[]) {
            if (funded.has(sym)) continue;
            funded.add(sym);
            const t = TOKENS[sym];
            const slot = await pub.request({
                method: "eth_getStorageAt" as never,
                params: [t.address, "0x0", "latest"] as never,
            }).then(() => null).catch(() => null);
            void slot;
            const key = (await import("viem")).keccak256(
                (await import("viem")).encodeAbiParameters(
                    [{ type: "address" }, { type: "uint256" }],
                    [account.address, BigInt(t.slot)],
                ),
            );
            await pub.request({
                method: "anvil_setStorageAt" as never,
                params: [
                    t.address,
                    key,
                    `0x${(parseUnits(process.env.FUND_UNITS ?? "1000000", t.decimals)).toString(16).padStart(64, "0")}`,
                ] as never,
            });
            // USDT reverts on changing a non-zero allowance, so clear it first. And UNI rejects any
            // allowance that does not fit uint96, so stay below that rather than using the max.
            await send(pos.approve({ token: t.address as Address, amount: 0n }));
            await send(pos.approve({ token: t.address as Address, amount: 2n ** 95n }));
            console.log(`funded + approved ${sym}`);
        }
    }

    for (const p of POSITIONS) {
        const base = TOKENS[p.base];
        const quote = TOKENS[p.quote];
        const built = Strategy.concentrated({
            base: { address: base.address as Address, decimals: base.decimals },
            quote: { address: quote.address as Address, decimals: quote.decimals },
            priceMin: p.priceMin,
            priceMax: p.priceMax,
        })
            .fee(p.feeBps)
            .salt(BigInt(Date.now()))
            .build(account.address);

        await send(
            pos.ship({
                strategy: built.order,
                amounts: [
                    { token: base.address as Address, amount: parseUnits(p.baseAmount, base.decimals) },
                    { token: quote.address as Address, amount: parseUnits(p.quoteAmount, quote.decimals) },
                ],
            }),
        );
        console.log(
            `shipped ${p.base}/${p.quote}  ${p.baseAmount}/${p.quoteAmount}  band ${p.priceMin}-${p.priceMax}  hash ${built.strategyHash}`,
        );
    }
    console.log(`maker ${account.address}`);
}

main().catch((e) => {
    console.error(e);
    process.exit(1);
});
