import {
    isAddressEqual,
    isHex,
    size,
    type Address,
    type Hex,
} from "viem";

import type { Rebate, SolventClient } from "../client";
import { createWalletSession, type WalletClients } from "../swap/wallet";
import {
    InputValidationError,
    validatedAddress,
    validatedUint,
} from "../validation";

export interface RebateClientConfig extends WalletClients {
    api: Pick<SolventClient, "config" | "rebateDetail">;
}

export interface ExecuteRebateRequest {
    executor: Address;
    rebateId: string;
}

export interface ExecutedRebate {
    rebateId: Hex;
    transactionHash: Hex;
}

export interface RebateIntent {
    submit(): Promise<ExecutedRebate>;
}

export interface RebateClient {
    createIntent(request: ExecuteRebateRequest): RebateIntent;
}

export class RebateUnavailableError extends Error {
    constructor(message = "Rebate is no longer executable") {
        super(message);
        this.name = "RebateUnavailableError";
    }
}

interface RebatePlan {
    rebateId: Hex;
    executor: Address;
    chainId: number;
    tokenIn: Address;
    filler: Address;
    deposit: bigint;
    deadlineBlock: bigint;
    calldata: Hex;
}

/** Execute immutable server-authorized work through a caller-owned wallet. */
export function createRebateClient({
    api,
    ...clients
}: RebateClientConfig): RebateClient {
    const wallet = createWalletSession(clients);

    function createIntent(request: ExecuteRebateRequest): RebateIntent {
        const snapshot = {
            executor: validatedAddress(request.executor, "rebate executor"),
            rebateId: validatedHash(request.rebateId, "rebate id"),
        };
        let plan: Promise<RebatePlan> | undefined;
        let transactionHash: Hex | undefined;
        let result: ExecutedRebate | undefined;
        let pending: Promise<ExecutedRebate> | undefined;

        async function prepare(): Promise<RebatePlan> {
            const [config, rebate] = await Promise.all([
                api.config(),
                api.rebateDetail(snapshot.rebateId),
            ]);
            const current = executablePlan(snapshot, config, rebate);
            await assertLive(current);
            return current;
        }

        function executionPlan(): Promise<RebatePlan> {
            plan ??= prepare().catch((error: unknown) => {
                plan = undefined;
                throw error;
            });
            return plan;
        }

        async function assertLive(current: RebatePlan): Promise<void> {
            const block = await wallet.currentBlock(current.chainId);
            // A transaction submitted at the deadline can only mine in a later, invalid block.
            if (block >= current.deadlineBlock) {
                throw new RebateUnavailableError("Rebate authorization expired");
            }
        }

        async function execute(): Promise<ExecutedRebate> {
            if (result) return result;
            const current = await executionPlan();
            if (!transactionHash) {
                await wallet.ensureAllowance({
                    owner: current.executor,
                    chainId: current.chainId,
                    token: current.tokenIn,
                    spender: current.filler,
                    amount: current.deposit,
                });
                await assertLive(current);
                transactionHash = await wallet.sendTransaction({
                    owner: current.executor,
                    chainId: current.chainId,
                    to: current.filler,
                    data: current.calldata,
                    value: 0n,
                });
            }
            await wallet.confirmTransaction(transactionHash);
            result = {
                rebateId: current.rebateId,
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

    return { createIntent };
}

function executablePlan(
    request: { executor: Address; rebateId: Hex },
    config: Awaited<ReturnType<SolventClient["config"]>>,
    rebate: Rebate,
): RebatePlan {
    if (rebate.status !== "ready") throw new RebateUnavailableError();
    if (rebate.id.toLowerCase() !== request.rebateId.toLowerCase()) {
        throw invalid("rebate id", "Server returned a different rebate");
    }
    const filler = validatedAddress(config.filler, "configured filler");
    const target = validatedAddress(required(rebate.to, "rebate target"), "rebate target");
    if (!isAddressEqual(filler, target)) {
        throw invalid("rebate target", "Rebate target does not match the configured filler");
    }
    const tokenIn = validatedAddress(rebate.token_in, "rebate input token");
    const tokenOut = validatedAddress(rebate.token_out, "rebate output token");
    if (isAddressEqual(tokenIn, tokenOut)) {
        throw invalid("rebate tokens", "Rebate tokens must be distinct");
    }
    const amountIn = wireUint(rebate.amount_in, "rebate input amount", true);
    const makerRebate = wireUint(rebate.maker_rebate, "maker rebate", true);
    const deposit = validatedUint(
        amountIn + makerRebate,
        256,
        "rebate deposit",
        { positive: true },
    );
    const deadlineBlock = wireBlock(rebate.deadline_block);
    const calldata = required(rebate.calldata, "rebate calldata");
    if (!isHex(calldata, { strict: true }) || size(calldata) < 4) {
        throw invalid("rebate calldata", "Rebate calldata must include a function selector");
    }
    return {
        rebateId: request.rebateId,
        executor: request.executor,
        chainId: config.chain_id,
        tokenIn,
        filler,
        deposit,
        deadlineBlock,
        calldata,
    };
}

function wireUint(value: string, field: string, positive = false): bigint {
    try {
        return validatedUint(BigInt(value), 256, field, { positive });
    } catch (error) {
        if (error instanceof InputValidationError) throw error;
        throw invalid(field, `${field} must be an unsigned integer`);
    }
}

function wireBlock(value: number | null | undefined): bigint {
    if (!Number.isSafeInteger(value) || (value ?? 0) <= 0) {
        throw invalid("rebate deadline", "Rebate deadline must be a positive safe integer");
    }
    return BigInt(value as number);
}

function validatedHash(value: string, field: string): Hex {
    if (!isHex(value, { strict: true }) || size(value) !== 32) {
        throw invalid(field, `${field} must contain exactly 32 bytes`);
    }
    return value;
}

function required<T>(value: T | null | undefined, field: string): T {
    if (value === null || value === undefined) {
        throw invalid(field, `${field} is missing`);
    }
    return value;
}

function invalid(field: string, message: string): InputValidationError {
    return new InputValidationError(field, "out_of_range", message);
}
