import type { components, operations } from "./generated/types";
import { SolventApiError, SolventNetworkError } from "./errors";
import { defaultTransport, type Transport } from "./transport";

type Schemas = components["schemas"];

// The DTOs consumers read, re-exported from the generated wire types under API-facing names.
export type AppConfig = Schemas["AppConfig"];
export type Asset = Schemas["Asset"];
export type Pool = Schemas["Pool"];
export type PoolDetail = Schemas["PoolDetail"];
export type PoolDepth = Schemas["PoolDepth"];
export type Stats = Schemas["Stats"];
export type QuoteRequest = Schemas["QuoteRequest"];
export type QuoteResponse = Schemas["QuoteResponse"];
export type SwapRequest = Schemas["SwapRequest"];
export type SwapResponse = Schemas["SwapResponse"];
export type Trade = Schemas["TradeView"];
export type ObservedOrders = Schemas["ObservedOrders"];
export type ObservedOrder = Schemas["ObservedOrder"];
export type ActivityEvent = Schemas["ActivityEvent"];
export type MakerSummary = Schemas["MakerSummary"];
export type MakerDashboard = Schemas["MakerDashboard"];
export type MakerTrade = Schemas["MakerTrade"];
export type InventoryRow = Schemas["InventoryRow"];
export type Position = Schemas["Position"];
export type PositionHistory = Schemas["PositionHistory"];
export type TokenBalance = Schemas["TokenBalance"];
export type PairInfo = Schemas["PairInfo"];
export type PairPriceHistory = Schemas["PairPriceHistory"];
export type PriceHistoryPeriod = Schemas["PriceHistoryPeriod"];
export type PreviewRequest = Schemas["PreviewRequest"];
export type PreviewResponse = Schemas["PreviewResponse"];
export type Rebate = Schemas["RebateWork"];
export type RebateStatus = Schemas["RebateStatus"];
export type RebateAllocation = Schemas["RebateAllocationView"];

/** A page of a collection (mirrors the wire `List<T>`). */
export interface List<T> {
    items: T[];
    next_cursor?: string;
    total?: number;
}

export interface SolventClientConfig {
    /** API origin, e.g. `https://api.solvent.example` (a trailing slash is fine). */
    baseUrl: string;
    /** Injected transport (auth/retry/mock); defaults to the platform `fetch`. */
    transport?: Transport;
    /** Headers sent on every request. */
    headers?: Record<string, string>;
}

export type MakerQuery = NonNullable<
    operations["maker_dashboard"]["parameters"]["query"]
>;

export type MakerTradesQuery = NonNullable<
    operations["maker_trades"]["parameters"]["query"]
>;
export type TradesQuery = NonNullable<
    operations["trades"]["parameters"]["query"]
>;
export type ActivityQuery = NonNullable<
    operations["activity"]["parameters"]["query"]
>;
export type PoolDepthQuery = NonNullable<
    operations["pool_depth"]["parameters"]["query"]
>;
export type PositionDepthQuery = NonNullable<
    operations["position_depth"]["parameters"]["query"]
>;
export type PairHistoryQuery = NonNullable<
    operations["pair_history"]["parameters"]["query"]
>;
export type RebatesQuery = Omit<
    NonNullable<operations["rebates"]["parameters"]["query"]>,
    "status"
> & { status?: RebateStatus };
type Query = Record<string, string | number | boolean | undefined>;

/** The typed read/write client over the Solvent API. Every method throws {@link SolventApiError}
 * on a failed response and {@link SolventNetworkError} when the transport itself fails. */
export interface SolventClient {
    config(): Promise<AppConfig>;
    assets(query?: { supported?: boolean }): Promise<List<Asset>>;
    pools(): Promise<List<Pool>>;
    poolDetail(query: { base: string; quote: string }): Promise<PoolDetail>;
    poolDepth(query: PoolDepthQuery): Promise<PoolDepth>;
    quote(body: QuoteRequest): Promise<QuoteResponse>;
    swap(body: SwapRequest): Promise<SwapResponse>;
    /** Every order the feed showed the resolver, newest first, narrowed by the filter fields. */
    orders(query?: {
        limit?: number;
        offset?: number;
        source?: string;
        token_in?: string;
        token_out?: string;
        state?: string;
    }): Promise<ObservedOrders>;
    trades(query?: TradesQuery): Promise<List<Trade>>;
    tradeDetail(id: string): Promise<Trade>;
    activity(query?: ActivityQuery): Promise<List<ActivityEvent>>;
    stats(): Promise<Stats>;
    makers(): Promise<List<MakerSummary>>;
    maker(maker: string, query?: MakerQuery): Promise<MakerDashboard>;
    makerInventory(
        maker: string,
        query?: MakerQuery,
    ): Promise<List<InventoryRow>>;
    makerTrades(
        maker: string,
        query?: MakerTradesQuery,
    ): Promise<List<MakerTrade>>;
    makerPositions(maker: string, query?: MakerQuery): Promise<List<Position>>;
    position(hash: string): Promise<Position>;
    positionDepth(hash: string, query?: PositionDepthQuery): Promise<PoolDepth>;
    positionHistory(hash: string): Promise<PositionHistory>;
    balances(wallet: string): Promise<List<TokenBalance>>;
    pairs(query?: {
        search?: string;
        wallet?: string;
    }): Promise<List<PairInfo>>;
    pairHistory(query: PairHistoryQuery): Promise<PairPriceHistory>;
    positionsPreview(body: PreviewRequest): Promise<PreviewResponse>;
    rebates(query?: RebatesQuery): Promise<List<Rebate>>;
    rebateDetail(id: string): Promise<Rebate>;
}

/** Create a client bound to `baseUrl`. Reads and the two writes go through one `Transport`. */
export function createSolventClient(
    config: SolventClientConfig,
): SolventClient {
    const transport = config.transport ?? defaultTransport;
    const base = config.baseUrl.replace(/\/$/, "");

    async function request<T>(
        method: string,
        path: string,
        opts?: { query?: Query; body?: unknown },
    ): Promise<T> {
        const hasBody = opts?.body !== undefined;
        const init: RequestInit = {
            method,
            headers: {
                ...(hasBody ? { "content-type": "application/json" } : {}),
                ...config.headers,
            },
            ...(hasBody ? { body: JSON.stringify(opts?.body) } : {}),
        };

        let response: Response;
        try {
            response = await transport(
                base + path + queryString(opts?.query),
                init,
            );
        } catch (cause) {
            throw new SolventNetworkError(`request to ${path} failed`, {
                cause,
            });
        }

        const envelope = (await response.json().catch(() => ({}))) as {
            result?: T;
            error?: string | null;
        };
        if (!response.ok || envelope.error != null) {
            throw new SolventApiError(
                response.status,
                envelope.error ?? response.statusText,
                envelope.error ?? undefined,
            );
        }
        return envelope.result as T;
    }

    const get = <T>(path: string, query?: Query) =>
        request<T>("GET", path, { query });
    const post = <T>(path: string, body: unknown) =>
        request<T>("POST", path, { body });

    return {
        config: () => get<AppConfig>("/v1/config"),
        assets: (query) => get<List<Asset>>("/v1/assets", query),
        pools: () => get<List<Pool>>("/v1/pools"),
        poolDetail: (query) => get<PoolDetail>("/v1/pools/detail", query),
        poolDepth: (query) => get<PoolDepth>("/v1/pools/depth", query),
        quote: (body) => post<QuoteResponse>("/v1/swap/quote", body),
        swap: (body) => post<SwapResponse>("/v1/swap", body),
        orders: (query) => get<ObservedOrders>("/v1/orders", query),
        trades: (query) => get<List<Trade>>("/v1/trades", query),
        tradeDetail: (id) => get<Trade>(`/v1/trades/${id}`),
        activity: (query) => get<List<ActivityEvent>>("/v1/activity", query),
        stats: () => get<Stats>("/v1/stats"),
        makers: () => get<List<MakerSummary>>("/v1/makers"),
        maker: (maker, query) =>
            get<MakerDashboard>(`/v1/makers/${maker}`, query),
        makerInventory: (maker, query) =>
            get<List<InventoryRow>>(`/v1/makers/${maker}/inventory`, query),
        makerTrades: (maker, query) =>
            get<List<MakerTrade>>(`/v1/makers/${maker}/trades`, query),
        makerPositions: (maker, query) =>
            get<List<Position>>(`/v1/makers/${maker}/positions`, query),
        position: (hash) => get<Position>(`/v1/positions/${hash}`),
        positionDepth: (hash, query) =>
            get<PoolDepth>(`/v1/positions/${hash}/depth`, query),
        positionHistory: (hash) =>
            get<PositionHistory>(`/v1/positions/${hash}/history`),
        balances: (wallet) =>
            get<List<TokenBalance>>(`/v1/wallets/${wallet}/balances`),
        pairs: (query) => get<List<PairInfo>>("/v1/pairs", query),
        pairHistory: (query) =>
            get<PairPriceHistory>("/v1/pairs/history", query),
        positionsPreview: (body) =>
            post<PreviewResponse>("/v1/positions/preview", body),
        rebates: (query) => get<List<Rebate>>("/v1/rebates", query),
        rebateDetail: (id) => get<Rebate>(`/v1/rebates/${id}`),
    };
}

function queryString(query?: Query): string {
    if (!query) return "";
    const params = new URLSearchParams();
    for (const [key, value] of Object.entries(query)) {
        if (value !== undefined) params.set(key, String(value));
    }
    const s = params.toString();
    return s ? `?${s}` : "";
}
