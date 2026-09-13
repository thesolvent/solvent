import type { PairRef } from "./index";
export type MakerPeriod = "7d" | "1m" | "3m" | "6m";

export interface ActivityBucket {
  from: number;
  to: number;
  fills: number;
  latencyMs: number | null;
}

import type { TradeRecord } from "./explorer";

export interface MakerSummary {
  address: string;
  activePositions: number;
  sharedLiquidityUsd: number | null;
}

export interface MakerDashboard extends MakerSummary {
  windowDays: number;
  volumeUsd: number | null;
  feesUsd: number | null;
  walletUsd: number | null;
  pullableUsd: number | null;
  coverage: number | null;
  liquidityChangePct: number | null;
  volumeChangePct: number | null;
  feesChangePct: number | null;
  fills: number;
  pairFills: number;
  sharePct: number | null;
  latencyMs: number | null;
  previousLatencyMs: number | null;
  fillsChangePct: number | null;
  activity: ActivityBucket[];
  insight: string | null;
}

export interface ValuedAmount {
  display: string;
  usd: number | null;
}

export interface PositionBalance extends ValuedAmount {
  address: string;
  symbol: string;
}

export interface PositionToken {
  address: string;
  decimals: number;
  symbol: string;
}

export interface Position {
  ref: PairRef;
  base: PositionToken;
  quote: PositionToken;
  hash: string;
  maker: string;
  pair: string;
  quoteSymbol: string;
  curve: string;
  state: string;
  feeBps: number;
  range: string;
  rangeKind: string;
  lowerPrice: number | null;
  upperPrice: number | null;
  belowPct: number | null;
  abovePct: number | null;
  pairType: string;
  spot: string | null;
  committed: PositionBalance[];
  actual: PositionBalance[];
  opening: PositionBalance[];
  committedUsd: number | null;
  actualUsd: number | null;
  coverage: number | null;
  backed: boolean;
  shortfall: string | null;
  feesUsd: number | null;
  apyPct: number | null;
  volumeUsd: number | null;
  fills7d: number | null;
  dailyFills: number[];
  volume7dUsd: number | null;
  lastFillAt: number | null;
  quoteUptimePct: number | null;
}

export interface InventoryLeg {
  hash: string;
  pair: string;
  curve: string;
  feeBps: number;
  current: ValuedAmount;
  opening: ValuedAmount;
  feesUsd: number | null;
  apyPct: number | null;
  coverage: number | null;
}

export interface InventoryAsset {
  address: string;
  symbol: string;
  logoUri?: string | null;
  wallet: ValuedAmount;
  shared: ValuedAmount;
  feesUsd: number | null;
  apyPct: number | null;
  legs: InventoryLeg[];
}

export interface MakerSettlement extends TradeRecord {
  feeUsd: number | null;
  sharePct: number | null;
}

export interface PositionHistory {
  from: number;
  to: number;
  createdBlock: number | null;
  prices: { at: number; price: number | null }[];
}
