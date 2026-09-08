import {
    createPublicClient,
    createTestClient,
    createWalletClient,
    custom,
    http,
    parseAbi,
    parseEther,
    type Address,
} from "viem";
import { generatePrivateKey, privateKeyToAccount } from "viem/accounts";
import { anvil } from "viem/chains";
import { describe, expect, it } from "vitest";
import { createSolventClient } from "../../src/client";
import { createSwapClient } from "../../src/swap";

const API_URL = process.env.SOLVENT_API_URL;
const RPC_URL = process.env.SOLVENT_RPC_URL ?? "http://127.0.0.1:8545";

describe.runIf(API_URL)("wallet RPC swap settlement", () => {
    it("settles a funded wallet's signed swap and transfers the promised tokens", async () => {
        const api = createSolventClient({ baseUrl: API_URL! });
        const publicClient = createPublicClient({
            chain: anvil,
            transport: http(RPC_URL),
        });
        const config = await api.config();
        expect(config.chain_id).toBe(anvil.id);
        expect(await publicClient.getChainId()).toBe(anvil.id);
        const assets = await api.assets();
        const token = (symbol: string) =>
            assets.items.find((asset) => asset.symbol === symbol)!
                .address as Address;
        const input = token("DAI");
        const output = token("USDC");
        const amount = 5_000n * 10n ** 18n;
        const taker = privateKeyToAccount(generatePrivateKey());
        const signer = createWalletClient({
            account: taker,
            chain: anvil,
            transport: http(RPC_URL),
        });
        const devnet = createTestClient({
            mode: "anvil",
            chain: anvil,
            transport: http(RPC_URL),
        });
        await devnet.setBalance({
            address: taker.address,
            value: parseEther("1"),
        });
        const mint = await signer.writeContract({
            address: input,
            abi: parseAbi(["function mint(address to, uint256 amount)"]),
            functionName: "mint",
            args: [taker.address, amount],
        });
        expect(
            (await publicClient.waitForTransactionReceipt({ hash: mint }))
                .status,
        ).toBe("success");
        const quote = await api.quote({
            token_in: input,
            token_out: output,
            amount_in: amount.toString(),
        });
        const minimum = (BigInt(quote.amount_out.raw) * 99n) / 100n;
        // Traverse the wallet JSON-RPC boundary, where a direct local signer hides BigNumber objects.
        const wallet = createWalletClient({
            account: taker.address,
            chain: anvil,
            transport: custom({
                request: async ({ method, params }) => {
                    if (method === "eth_accounts") return [taker.address];
                    if (method === "eth_sendTransaction") {
                        const tx = params[0];
                        return signer.sendTransaction({
                            to: tx.to,
                            data: tx.data,
                            value: tx.value ? BigInt(tx.value) : undefined,
                        });
                    }
                    if (method !== "eth_signTypedData_v4") {
                        return publicClient.transport.request({
                            method,
                            params,
                        });
                    }
                    const payload = JSON.parse(params[1]);
                    expect(payload.message.permitted.amount).toBe(
                        amount.toString(),
                    );
                    return taker.signTypedData(payload);
                },
            }),
        });
        const swaps = createSwapClient({
            api,
            publicClient,
            walletClient: wallet,
        });
        const balance = async (token: Address) =>
            (
                await swaps.tokenAccount({
                    token,
                    owner: taker.address,
                    spender: config.permit2 as Address,
                })
            ).balance;
        const beforeInput = await balance(input);
        const beforeOutput = await balance(output);
        const intent = swaps.createIntent({
            swapper: taker.address,
            tokenIn: input,
            tokenOut: output,
            amountIn: amount,
            minAmountOut: minimum,
            deadline: Math.floor(Date.now() / 1000) + 600,
        });
        const submitted = await intent.submit();
        expect(submitted.status).toBe("submitted");
        await expect
            .poll(
                async () => (await api.tradeDetail(submitted.trade_id)).status,
                { timeout: 45_000, interval: 1000 },
            )
            .toBe("confirmed");
        expect(beforeInput - (await balance(input))).toBe(amount);
        expect((await balance(output)) - beforeOutput).toBeGreaterThanOrEqual(
            minimum,
        );
    }, 90_000);
});
