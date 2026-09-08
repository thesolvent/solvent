import assert from "node:assert/strict";
import { test } from "node:test";

import { parseUnits, type Address, type Hex } from "viem";
import { privateKeyToAccount } from "viem/accounts";

import type { Devnet } from "../lib/devnet.ts";
import { Strategy, positions } from "../lib/node-runtime.ts";
import { seedPair } from "./strategies.ts";

const KEY =
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const maker = privateKeyToAccount(KEY);
const base = {
    address: "0x2222222222222222222222222222222222222222" as Address,
    decimals: 18,
};
const quote = {
    address: "0x3333333333333333333333333333333333333333" as Address,
    decimals: 6,
};
const aqua = "0x4444444444444444444444444444444444444444" as Address;
const app = "0x5555555555555555555555555555555555555555" as Address;
const spec = { base: "WETH", quote: "USDC", widthPct: 8, feeBps: 5, size: 30 };
const pricing = { kind: "ranged" as const, mid: 3000 };
const curve = Strategy.inRange({
    base,
    quote,
    mid: "3000",
    halfWidthPct: 8,
}).fee(5);
const copies = [0n, 1n, 2n, 3n].map((salt) =>
    curve.salt(salt).build(maker.address),
);
const pos = positions({ aqua, app });
const ships = copies.map((copy) =>
    pos.ship({
        strategy: copy.order,
        amounts: [
            { token: base.address, amount: parseUnits("30", 18) },
            { token: quote.address, amount: parseUnits("90000", 6) },
        ],
    }),
);

for (const { name, counts, pending } of [
    {
        name: "replaces docked identities with two active copies",
        counts: [255, 255],
        pending: [2, 3],
    },
    {
        name: "retains an active copy while replacing a docked identity",
        counts: [255, 2],
        pending: [2],
    },
    { name: "leaves two active copies untouched", counts: [2, 2], pending: [] },
]) {
    test(`seedPair ${name}`, async () => {
        const active = new Map<Hex, number>(
            copies.map((copy, i) => [copy.strategyHash, counts[i] ?? 0]),
        );
        const minted: { token: Address; amount: bigint }[] = [];
        const sent: { to: Address; data: Hex; value: bigint }[] = [];
        const env = {
            manifest: {
                aqua,
                router: app,
                tokens: { WETH: base, USDC: quote },
            },
            publicClient: {
                async readContract({
                    args,
                }: {
                    args: [Address, Address, Hex, Address];
                }) {
                    assert.equal(args[0], maker.address);
                    assert.equal(args[1], app);
                    assert.equal(args[3], base.address);
                    // An active strategy can have no balance left in this particular token.
                    return [0n, active.get(args[2]) ?? 0];
                },
            },
            wallet(account: { address: Address }) {
                assert.equal(account.address, maker.address);
                return {
                    async mint(token: Address, amount: bigint) {
                        minted.push({ token, amount });
                    },
                    async send(tx: { to: Address; data: Hex; value: bigint }) {
                        sent.push(tx);
                        const index = ships.findIndex(
                            (ship) => ship.data === tx.data,
                        );
                        if (index !== -1) {
                            assert.equal(
                                active.get(copies[index].strategyHash),
                                0,
                            );
                            active.set(copies[index].strategyHash, 2);
                        }
                    },
                };
            },
        } as unknown as Devnet;

        await seedPair(env, spec, KEY, pricing);

        assert.deepEqual(
            sent,
            pending.length === 0
                ? []
                : [
                      pos.approve({ token: base.address, amount: 2n ** 255n }),
                      pos.approve({ token: quote.address, amount: 2n ** 255n }),
                      ...pending.map((i) => ships[i]),
                  ],
        );
        assert.deepEqual(
            minted,
            pending.length === 0
                ? []
                : [
                      {
                          token: base.address,
                          amount: parseUnits("1000000", 18),
                      },
                      {
                          token: quote.address,
                          amount: parseUnits("1000000", 6),
                      },
                  ],
        );
        assert.equal(
            [...active.values()].filter((count) => count > 0 && count < 255)
                .length,
            2,
        );

        sent.length = 0;
        minted.length = 0;
        await seedPair(env, spec, KEY, pricing);
        assert.deepEqual(sent, []);
        assert.deepEqual(minted, []);
    });
}
