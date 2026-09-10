import { describe, expect, it, vi } from "vitest";
import { createCrossChainClient } from "../../src/cross-chain";

describe("cross-chain client", () => {
    it("submits an immutable hex-encoded plan", async () => {
        const fetcher = vi.fn(async () =>
            new Response(
                JSON.stringify({
                    order: { order_id: "0x01", state: "prepared" },
                }),
                { status: 200 },
            ),
        );
        const client = createCrossChainClient({
            baseUrl: "https://proxy.example/",
            fetch: fetcher as typeof fetch,
        });
        const request = {
            order_id: "0x01" as const,
            quote: {
                id: "0x02" as const,
                origin: {} as never,
                destination: {} as never,
                amount_in: "0x01" as const,
                amount_out: "0x01" as const,
                bridge_fee: "0x00" as const,
                expires_at_unix: 10,
            },
            origin_plan: {
                aggregate_id: "0x02" as const,
                chain_id: 1,
                steps: [],
            },
            destination_plan: {
                aggregate_id: "0x02" as const,
                chain_id: 2,
                steps: [],
            },
        };
        await expect(client.submit(request)).resolves.toMatchObject({
            state: "prepared",
        });
        expect(fetcher).toHaveBeenCalledWith(
            "https://proxy.example/v1/cross-chain/orders",
            expect.objectContaining({ method: "POST" }),
        );
    });
});
