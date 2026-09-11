import type { SolventClient, SwapRequest, SwapResponse } from "../client";
import {
    assertFutureDeadline,
    buildSwapOrder,
    type OrderTerms,
} from "../orders";
import {
    createWalletSession,
    type TokenAccount,
    type TokenAccountRequest,
    type WalletActionOptions,
    type WalletActionStatus,
    type WalletClients,
} from "./wallet";

export interface SwapClientConfig extends WalletClients {
    api: Pick<SolventClient, "config" | "swap">;
}

/** One payment authorization. Reuse this object to retry an uncertain submission. */
export interface SwapIntent {
    submit(options?: SwapSubmissionOptions): Promise<SwapResponse>;
}

export type SwapSubmissionStatus = { kind: "preparing" } | WalletActionStatus;

export interface SwapSubmissionOptions extends WalletActionOptions {
    onStatus?(status: SwapSubmissionStatus): void;
}

export interface SwapClient {
    tokenAccount(request: TokenAccountRequest): Promise<TokenAccount>;
    createIntent(terms: OrderTerms): SwapIntent;
}

export class SwapDeclinedError extends Error {
    constructor() {
        super("The resolver declined this swap");
        this.name = "SwapDeclinedError";
    }
}

/** Bind API and wallet dependencies once; each intent owns its signing and retry state. */
export function createSwapClient({
    api,
    ...clients
}: SwapClientConfig): SwapClient {
    const wallet = createWalletSession(clients);

    async function authorize(
        terms: OrderTerms,
        options?: SwapSubmissionOptions,
    ): Promise<SwapRequest> {
        const config = await api.config();
        const order = buildSwapOrder(
            {
                chainId: config.chain_id,
                reactor: config.reactor,
                permit2: config.permit2,
                cosigner: config.cosigner,
            },
            terms,
        );
        const signature = await wallet.sign(order, options);
        return {
            encodedOrder: order.encodedOrder,
            signature,
            chainId: config.chain_id,
        };
    }

    function createIntent(terms: OrderTerms): SwapIntent {
        // Callers may edit their form while wallet requests or HTTP submissions are pending.
        const snapshot = { ...terms };
        let signed: SwapRequest | undefined;
        let result: SwapResponse | undefined;
        let pending: Promise<SwapResponse> | undefined;

        async function execute(
            options?: SwapSubmissionOptions,
        ): Promise<SwapResponse> {
            options?.onStatus?.({ kind: "preparing" });
            signed ??= await authorize(snapshot, options);
            assertFutureDeadline(snapshot.deadline);
            options?.onStatus?.({ kind: "submitting" });
            result ??= await api.swap(signed);
            if (result.status === "declined") throw new SwapDeclinedError();
            return result;
        }

        return {
            submit(options) {
                // Share in-flight work and retain signed bytes after an ambiguous HTTP failure.
                pending ??= execute(options).finally(() => {
                    pending = undefined;
                });
                return pending;
            },
        };
    }

    return { tokenAccount: wallet.tokenAccount, createIntent };
}
