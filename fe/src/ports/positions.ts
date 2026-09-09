import type { WalletClients } from "@solvent/sdk/swap";

export interface CreateToken {
  address: `0x${string}`;
  decimals: number;
  symbol: string;
  name: string;
  tags: string[];
  balance: number;
  balanceRaw: bigint;
  valueUsd: number | undefined;
  changePct: number | undefined;
  tint: string;
}

export interface CreatePair {
  base: CreateToken;
  quote: CreateToken;
  mid: number;
  tvlUsd: number | undefined;
  type: "Stable" | "Volatile";
  defaultBandPct: number;
  defaultFeeBps: number;
}

export type PositionCurve = "Concentrated" | "Pegged" | "Full range";

export interface PositionForm {
  pair: CreatePair;
  flipped?: boolean;
  curve: PositionCurve;
  feeBps: number;
  spotPrice: string;
  priceMin: string;
  priceMax: string;
  halfWidthPct: number;
  peggedSymmetric?: boolean;
  amountBase: string;
  amountQuote: string;
}

export interface PositionInput extends PositionForm {
  maker: `0x${string}`;
}

export interface CreatedPosition {
  strategyHash: string;
  transactionHash: string;
}

export type PriceHistoryPeriod = "7d" | "3m" | "all";

export interface PairPricePoint {
  timestampMs: number;
  price: number;
  volumeUsd: number | undefined;
}

export interface PositionIntent {
  submit(): Promise<CreatedPosition>;
}

export interface PositionsPort {
  pairs(wallet?: string): Promise<CreatePair[]>;
  history(
    pair: CreatePair,
    period: PriceHistoryPeriod,
  ): Promise<PairPricePoint[]>;
  createIntent(input: PositionInput, clients: WalletClients): PositionIntent;
}
