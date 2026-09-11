export type RebateStatus = "ready" | "executed";

export interface RebateRecord {
  id: string;
  status: RebateStatus;
  maker: string;
  strategyHash: string;
  tokenIn: `0x${string}`;
  tokenOut: `0x${string}`;
  amountIn: bigint;
  amountOut: bigint;
  makerRebate: bigint;
  executorProfit: bigint;
  deviationBps: number;
  deadlineBlock: number | null;
  publishedAt: number | null;
  executedAt: number | null;
  transactionHash: string | null;
}

export interface ExecutedRebate {
  rebateId: string;
  transactionHash: string;
}
