import type {
    AggregateQuote,
    CreateCrossChainOrderRequest,
    CrossChainOrder,
    CrossChainQuoteRequest,
} from "./types";

export interface CrossChainClientConfig {
    baseUrl: string;
    fetch?: typeof globalThis.fetch;
    headers?: Record<string, string>;
}

export interface CrossChainClient {
    quote(request: CrossChainQuoteRequest): Promise<AggregateQuote>;
    submit(request: CreateCrossChainOrderRequest): Promise<CrossChainOrder>;
    order(orderId: string): Promise<CrossChainOrder>;
    wait(
        orderId: string,
        options?: { intervalMs?: number; signal?: AbortSignal },
    ): Promise<CrossChainOrder>;
}

export function createCrossChainClient({
    baseUrl,
    fetch: fetcher = globalThis.fetch,
    headers,
}: CrossChainClientConfig): CrossChainClient {
    const base = baseUrl.replace(/\/$/, "");

    async function request<T>(path: string, body?: unknown): Promise<T> {
        const response = await fetcher(base + path, {
            method: body === undefined ? "GET" : "POST",
            headers: {
                ...(body === undefined
                    ? {}
                    : { "content-type": "application/json" }),
                ...headers,
            },
            ...(body === undefined ? {} : { body: JSON.stringify(body) }),
        });
        const payload = (await response.json().catch(() => ({}))) as {
            error?: string;
        } & T;
        if (!response.ok) {
            throw new Error(payload.error ?? `Cross-chain HTTP ${response.status}`);
        }
        return payload;
    }

    async function order(orderId: string): Promise<CrossChainOrder> {
        const response = await request<{ order: CrossChainOrder }>(
            `/v1/cross-chain/orders/${orderId}`,
        );
        return response.order;
    }

    return {
        quote: (body) =>
            request<AggregateQuote>("/v1/cross-chain/quote", body),
        async submit(body) {
            const response = await request<{ order: CrossChainOrder }>(
                "/v1/cross-chain/orders",
                body,
            );
            return response.order;
        },
        order,
        async wait(orderId, options) {
            const interval = options?.intervalMs ?? 2_000;
            for (;;) {
                options?.signal?.throwIfAborted();
                const current = await order(orderId);
                if (
                    current.state === "complete" ||
                    current.state === "failed_before_delivery"
                ) {
                    return current;
                }
                await new Promise<void>((resolve, reject) => {
                    const timer = setTimeout(resolve, interval);
                    options?.signal?.addEventListener(
                        "abort",
                        () => {
                            clearTimeout(timer);
                            reject(options.signal?.reason);
                        },
                        { once: true },
                    );
                });
            }
        },
    };
}
