import type { Rebate } from "@solvent/sdk/client";
import type { RebateRecord } from "@/data/rebates";

function integer(value: string, field: string): bigint {
  let parsed: bigint;
  try {
    parsed = BigInt(value);
  } catch {
    throw new Error(`Invalid ${field} returned by the server`);
  }
  if (parsed < 0n) throw new Error(`Invalid ${field} returned by the server`);
  return parsed;
}

/** Convert wire integers once so UI code cannot accidentally do decimal math on raw strings. */
export function toRebate(api: Rebate): RebateRecord {
  return {
    id: api.id,
    status: api.status,
    maker: api.maker,
    strategyHash: api.strategy_hash,
    tokenIn: api.token_in as `0x${string}`,
    tokenOut: api.token_out as `0x${string}`,
    amountIn: integer(api.amount_in, "rebate input amount"),
    amountOut: integer(api.amount_out, "rebate output amount"),
    makerRebate: integer(api.maker_rebate, "maker rebate"),
    executorProfit: integer(api.executor_profit, "executor profit"),
    deviationBps: api.deviation_bps,
    deadlineBlock: api.deadline_block ?? null,
    publishedAt: api.published_at ?? null,
    executedAt: api.executed_at ?? null,
    transactionHash: api.transaction_hash ?? null,
  };
}
