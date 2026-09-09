import {
    getAddress,
    isHex,
    isAddressEqual,
    maxUint256,
    size,
    type Address,
    type Hex,
} from "viem";

import type { SolventClient } from "../client";
import type { BuiltStrategy } from "../construction/strategy";
import { createWalletSession, type WalletClients } from "../swap/wallet";
import {
    assertDistinctAddresses,
    InputValidationError,
    validatedAddress,
    validatedUint,
} from "../validation";
import { positions, type TxRequest } from "./builder";

export interface PositionAmount {
    token: Address;
    amount: bigint;
}

export interface CreatePositionRequest {
    maker: Address;
    strategy: BuiltStrategy;
    amounts: readonly PositionAmount[];
}

export interface CreatedPosition {
    strategyHash: Hex;
    transactionHash: Hex;
}

export interface PositionIntent {
    submit(): Promise<CreatedPosition>;
}

export interface PositionTransaction {
    transactionHash: Hex;
}

export interface PositionTransactionIntent {
    submit(): Promise<PositionTransaction>;
}

export interface PushPositionRequest {
    maker: Address;
    strategyHash: Hex;
    token: Address;
    amount: bigint;
}

export interface DockPositionRequest {
    maker: Address;
    strategyHash: Hex;
    tokens: readonly Address[];
}

export interface PositionClientConfig extends WalletClients {
    api: Pick<SolventClient, "config" | "positionsPreview">;
}

export interface PositionClient {
    createIntent(request: CreatePositionRequest): PositionIntent;
    pushIntent(request: PushPositionRequest): PositionTransactionIntent;
    dockIntent(request: DockPositionRequest): PositionTransactionIntent;
}

export class PositionExistsError extends Error {
    constructor() {
        super("A position with this strategy already exists");
        this.name = "PositionExistsError";
    }
}

export class PositionPreviewError extends Error {
    readonly warnings: readonly string[];

    constructor(warnings: readonly string[]) {
        super(warnings.join(". "));
        this.name = "PositionPreviewError";
        this.warnings = warnings;
    }
}

interface PositionPlan {
    chainId: number;
    aqua: Address;
    app: Address;
    approvals: PositionAmount[];
}

interface PositionTransactionPlan {
    owner: Address;
    chainId: number;
    transaction: TxRequest;
}

/** Coordinate server preflight, reusable Aqua approval, and a confirmed `ship` transaction. */
export function createPositionClient({
    api,
    ...clients
}: PositionClientConfig): PositionClient {
    const wallet = createWalletSession(clients);

    function actionIntent(
        prepare: () => Promise<PositionTransactionPlan>,
    ): PositionTransactionIntent {
        let plan: Promise<PositionTransactionPlan> | undefined;
        let transactionHash: Hex | undefined;
        let result: PositionTransaction | undefined;
        let pending: Promise<PositionTransaction> | undefined;

        function actionPlan() {
            plan ??= prepare().catch((error: unknown) => {
                plan = undefined;
                throw error;
            });
            return plan;
        }

        async function execute(): Promise<PositionTransaction> {
            if (result) return result;
            const current = await actionPlan();
            transactionHash ??= await wallet.sendTransaction({
                ...current.transaction,
                owner: current.owner,
                chainId: current.chainId,
            });
            await wallet.confirmTransaction(transactionHash);
            result = { transactionHash };
            return result;
        }

        return {
            submit() {
                pending ??= execute().finally(() => {
                    pending = undefined;
                });
                return pending;
            },
        };
    }

    function createIntent(request: CreatePositionRequest): PositionIntent {
        const snapshot = snapshotRequest(request);
        let plan: Promise<PositionPlan> | undefined;
        let transactionHash: Hex | undefined;
        let result: CreatedPosition | undefined;
        let pending: Promise<CreatedPosition> | undefined;

        async function prepare(): Promise<PositionPlan> {
            validateRequest(snapshot);
            const [config, preview] = await Promise.all([
                api.config(),
                api.positionsPreview({
                    maker: snapshot.maker,
                    strategy_hash: snapshot.strategy.strategyHash,
                    amounts: snapshot.amounts.map(({ token, amount }) => ({
                        token,
                        amount: amount.toString(),
                    })),
                }),
            ]);
            if (preview.exists) throw new PositionExistsError();
            if (preview.warnings.length > 0) {
                throw new PositionPreviewError(preview.warnings);
            }
            return {
                chainId: config.chain_id,
                aqua: getAddress(config.aqua),
                app: getAddress(config.app),
                approvals: requiredApprovals(
                    preview.requires_approval,
                    snapshot.amounts,
                ),
            };
        }

        function positionPlan(): Promise<PositionPlan> {
            plan ??= prepare().catch((error: unknown) => {
                plan = undefined;
                throw error;
            });
            return plan;
        }

        async function authorize(current: PositionPlan): Promise<void> {
            for (const approval of current.approvals) {
                await wallet.ensureAllowance({
                    owner: snapshot.maker,
                    chainId: current.chainId,
                    token: approval.token,
                    spender: current.aqua,
                    amount: approval.amount,
                    approvalAmount: maxUint256,
                });
            }
        }

        async function execute(): Promise<CreatedPosition> {
            if (result) return result;
            const current = await positionPlan();
            if (!transactionHash) {
                await authorize(current);
                const tx = positions(current).ship({
                    strategy: snapshot.strategy.order,
                    amounts: snapshot.amounts,
                });
                transactionHash = await wallet.sendTransaction({
                    ...tx,
                    owner: snapshot.maker,
                    chainId: current.chainId,
                });
            }
            await wallet.confirmTransaction(transactionHash);
            result = {
                strategyHash: snapshot.strategy.strategyHash,
                transactionHash,
            };
            return result;
        }

        return {
            submit() {
                pending ??= execute().finally(() => {
                    pending = undefined;
                });
                return pending;
            },
        };
    }

    function pushIntent(
        request: PushPositionRequest,
    ): PositionTransactionIntent {
        const snapshot = { ...request };
        return actionIntent(async () => {
            const maker = validatedAddress(snapshot.maker, "maker");
            const strategyHash = validatedStrategyHash(snapshot.strategyHash);
            const token = validatedAddress(snapshot.token, "push token");
            const amount = validatedUint(snapshot.amount, 256, "push amount", {
                positive: true,
            });
            const config = await api.config();
            const deployment = {
                chainId: config.chain_id,
                aqua: getAddress(config.aqua),
                app: getAddress(config.app),
            };
            await wallet.ensureAllowance({
                owner: maker,
                chainId: deployment.chainId,
                token,
                spender: deployment.aqua,
                amount,
                approvalAmount: maxUint256,
            });
            return {
                owner: maker,
                chainId: deployment.chainId,
                transaction: positions(deployment).push({
                    maker,
                    strategyHash,
                    token,
                    amount,
                }),
            };
        });
    }

    function dockIntent(
        request: DockPositionRequest,
    ): PositionTransactionIntent {
        const snapshot = { ...request, tokens: [...request.tokens] };
        return actionIntent(async () => {
            const maker = validatedAddress(snapshot.maker, "maker");
            const strategyHash = validatedStrategyHash(snapshot.strategyHash);
            const tokens = validatedPositionTokens(snapshot.tokens);
            const config = await api.config();
            const deployment = {
                chainId: config.chain_id,
                aqua: getAddress(config.aqua),
                app: getAddress(config.app),
            };
            return {
                owner: maker,
                chainId: deployment.chainId,
                transaction: positions(deployment).dock({
                    strategyHash,
                    tokens,
                }),
            };
        });
    }

    return { createIntent, pushIntent, dockIntent };
}

function snapshotRequest(
    request: CreatePositionRequest,
): CreatePositionRequest {
    return {
        maker: request.maker,
        strategy: { ...request.strategy },
        amounts: request.amounts.map((amount) => ({ ...amount })),
    };
}

function validateRequest(request: CreatePositionRequest): void {
    validateMaker(request);
    validateAmounts(request.amounts);
}

function requiredApprovals(
    requiredTokens: readonly string[],
    amounts: readonly PositionAmount[],
): PositionAmount[] {
    const required = new Set(
        requiredTokens.map((token) => token.toLowerCase()),
    );
    const known = new Set(amounts.map(({ token }) => token.toLowerCase()));
    if ([...required].some((token) => !known.has(token))) {
        throw new PositionPreviewError([
            "Server requested approval for a token outside the position",
        ]);
    }
    return amounts.filter(({ token }) => required.has(token.toLowerCase()));
}

function validateAmounts(amounts: readonly PositionAmount[]): void {
    if (amounts.length !== 2) {
        throw new InputValidationError(
            "position amounts",
            "out_of_range",
            "A position requires exactly two token amounts",
        );
    }
    const left = validatedAddress(amounts[0].token, "first position token");
    const right = validatedAddress(amounts[1].token, "second position token");
    assertDistinctAddresses(left, right, "position tokens");
    validatedUint(amounts[0].amount, 248, "first position amount", {
        positive: true,
    });
    validatedUint(amounts[1].amount, 248, "second position amount", {
        positive: true,
    });
}

function validateMaker(request: CreatePositionRequest): void {
    const maker = validatedAddress(request.maker, "maker");
    const encodedMaker = validatedAddress(
        request.strategy.maker,
        "strategy maker",
    );
    if (!isAddressEqual(maker, encodedMaker)) {
        throw new InputValidationError(
            "maker",
            "mismatched_address",
            "Position maker must match the maker encoded in the strategy",
        );
    }
}

function validatedStrategyHash(value: Hex): Hex {
    if (!isHex(value, { strict: true }) || size(value) !== 32) {
        throw new InputValidationError(
            "strategy hash",
            "out_of_range",
            "Strategy hash must contain exactly 32 bytes",
        );
    }
    return value;
}

function validatedPositionTokens(tokens: readonly Address[]): Address[] {
    if (tokens.length !== 2) {
        throw new InputValidationError(
            "position tokens",
            "out_of_range",
            "A position requires exactly two tokens",
        );
    }
    const left = validatedAddress(tokens[0], "first position token");
    const right = validatedAddress(tokens[1], "second position token");
    assertDistinctAddresses(left, right, "position tokens");
    return [left, right];
}
