export interface TokenQuantity {
  symbol: string;
  display: string;
}

export interface TradeStage {
  status: string;
  at: number;
}

export interface TradeSource {
  maker: string;
  strategyHash: string;
  input: TokenQuantity;
  output: TokenQuantity;
  sharePct: number;
}

export interface TradeRecord {
  id: string;
  status: string;
  taker: string;
  input: TokenQuantity;
  output: TokenQuantity;
  surplus: TokenQuantity | null;
  priceImpactPct: number | null;
  makers: number | null;
  txHash: string | null;
  blockNumber: number | null;
  createdAt: number;
  settledAt: number | null;
  deadlineAt: number | null;
  orderHash: string | null;
  lifecycle: TradeStage[];
  legs: TradeSource[];
}

export interface ActivityRecord {
  id: string;
  kind: string;
  maker: string;
  strategyHash: string;
  amount: TokenQuantity | null;
  at: number;
  blockNumber: number | null;
  txHash: string | null;
}

export interface ExplorerStats {
  blockHeight: number;
  events24h: number | null;
  tradesSettled: number | null;
  confirmedPct: number | null;
  medianImpactPct: number | null;
  activeMakers: number | null;
  quotingNow: number | null;
}

export interface RecordPage<T> {
  items: T[];
  nextCursor: string | undefined;
}
