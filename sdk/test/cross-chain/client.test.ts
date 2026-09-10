import { describe, expect, it, vi } from "vitest";
import {
    createCrossChainClient,
    CrossChainApiError,
} from "../../src/cross-chain";

describe("cross-chain client", () => {
    it("requests wallet-signable direct-order terms", async () => {
        const fetcher = vi.fn(async () =>
            new Response(
                JSON.stringify({
                    aggregate_id: "0x02",
                    order_id: "0x03",
                    compact: "0x0000000000000000000000000000000000000004",
                    order: {},
                    commitment: {},
                }),
                { status: 200 },
            ),
        );
        const client = createCrossChainClient({
            baseUrl: "https://proxy.example/",
            fetch: fetcher as typeof fetch,
        });

        await expect(
            client.draft({
                quote: {
                    id: "0x02",
                    origin: {} as never,
                    destination: {} as never,
                    amount_in: "0x01",
                    amount_out: "0x01",
                    bridge_fee: "0x00",
                    expires_at_unix: 10,
                },
                sponsor: "0x0000000000000000000000000000000000000001",
                recipient: "0x0000000000000000000000000000000000000002",
                order_nonce: "0x07",
                compact_nonce: "0x08",
                compact_expires_unix: 20,
            }),
        ).resolves.toMatchObject({ order_id: "0x03" });
        expect(fetcher).toHaveBeenCalledWith(
            "https://proxy.example/v1/cross-chain/orders/draft",
            expect.objectContaining({ method: "POST" }),
        );
    });

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

    it("submits wallet-authorized direct terms to the direct endpoint", async () => {
        const fetcher = vi.fn(async () =>
            new Response(
                JSON.stringify({
                    order: { order_id: "0x01", state: "destination_pending" },
                }),
                { status: 200 },
            ),
        );
        const client = createCrossChainClient({
            baseUrl: "https://proxy.example",
            fetch: fetcher as typeof fetch,
        });
        const request = {
            draft: {
                quote: {
                    id: "0x02" as const,
                    origin: {} as never,
                    destination: {} as never,
                    amount_in: "0x01" as const,
                    amount_out: "0x01" as const,
                    bridge_fee: "0x00" as const,
                    expires_at_unix: 10,
                },
                sponsor: "0x0000000000000000000000000000000000000001" as const,
                recipient: "0x0000000000000000000000000000000000000001" as const,
                order_nonce: "0x03" as const,
                compact_nonce: "0x04" as const,
                compact_expires_unix: 20,
            },
            sponsor_signature: "0x05" as const,
        };

        await expect(client.submitDirect(request)).resolves.toMatchObject({
            order_id: "0x01",
            state: "destination_pending",
        });
        expect(fetcher).toHaveBeenCalledWith(
            "https://proxy.example/v1/cross-chain/orders/direct",
            expect.objectContaining({
                method: "POST",
                body: JSON.stringify(request),
            }),
        );
    });

    it("retains the HTTP status for callers that distinguish missing orders", async () => {
        const client = createCrossChainClient({
            baseUrl: "https://proxy.example",
            fetch: vi.fn(async () =>
                new Response(JSON.stringify({ error: "order not found" }), {
                    status: 404,
                }),
            ) as typeof fetch,
        });

        await expect(client.order("0x01")).rejects.toEqual(
            new CrossChainApiError(404, "order not found"),
        );
    });
});
