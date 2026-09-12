export interface TokenQuantity {
  symbol: string;
  display: string;
  net?: string;
  logoUri?: string | null;
}

export interface TradeStage {
  status: string;
  at: number | null;
}

export interface TradeSource {
  curve: string | null;
  maker: string;
  strategyHash: string;
  chainId?: number;
  input: TokenQuantity;
  output: TokenQuantity;
  sharePct: number;
}

export interface TradeRecord {
  flow: "same-chain" | "cross-chain";
  signaturePresent: boolean | null;
  id: string;
  status: string;
  taker: string;
  input: TokenQuantity;
  output: TokenQuantity;
  surplus: TokenQuantity | null;
  /** What sourcing the delivery would have cost, gas included — present on declines too. */
  indicativeInput: TokenQuantity | null;
  /** Which venue the order arrived from: the public book, or our own endpoint. */
  source: string;
  /** Why this trade declined — the admission rule, the sim gate's real on-chain revert reason, or
   *  the margin call. */
  declineReason: string | null;
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

/** One order the feed showed the resolver, whatever became of it. */
export interface ObservedOrder {
  orderHash: string;
  source: string;
  tokenIn: string;
  tokenOut: string | null;
  amountIn: string;
  requiredOut: string | null;
  indicativeIn: string | null;
  verdict: string;
  reason: string | null;
  seenAt: number;
  /** The trade this order became, if it was ever attempted — `null` when it was refused at the
   *  door or declined as unprofitable before routing ever reserved anything. */
  tradeStatus: string | null;
  /** The trade's own id — what the explorer links to for the trade's full detail. */
  tradeId: string | null;
  tradeTxHash: string | null;
  /** Why the trade declined — the sim gate's real on-chain revert reason, or the margin call. */
  tradeDeclineReason: string | null;
}

/** One page of the order feed, plus how many orders the feed has ever shown us — the denominator
 *  this page sits inside. */
export interface ObservedOrderPage {
  items: ObservedOrder[];
  total: number;
}
