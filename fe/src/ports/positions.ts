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

/**
 * The two addresses a pair's history is keyed by.
 *
 * Narrower than `CreatePair`, which structurally satisfies it: the history read never needed the
 * curve defaults or wallet balances a create pair carries, and typing it that way kept the read
 * off every surface that only knows a pair's addresses.
 */
export interface PairAddresses {
  base: { address: string };
  quote: { address: string };
}

export interface PairPricePoint {
  timestampMs: number;
  price: number;
  volumeUsd: number | undefined;
}

export interface PositionIntent {
  submit(): Promise<CreatedPosition>;
}

export interface PositionTokenInput {
  address: string;
  decimals: number;
  symbol: string;
}

export interface PushPositionInput {
  maker: string;
  strategyHash: string;
  token: PositionTokenInput;
  amount: string;
}

export interface DockPositionInput {
  maker: string;
  strategyHash: string;
  tokens: readonly string[];
}

export interface PositionActionResult {
  transactionHash: string;
}

export interface PositionActionIntent {
  submit(): Promise<PositionActionResult>;
}

export interface PositionsPort {
  pairs(wallet?: string): Promise<CreatePair[]>;
  history(
    pair: PairAddresses,
    period: PriceHistoryPeriod,
  ): Promise<PairPricePoint[]>;
  createIntent(input: PositionInput, clients: WalletClients): PositionIntent;
  pushIntent(
    input: PushPositionInput,
    clients: WalletClients,
  ): PositionActionIntent;
  dockIntent(
    input: DockPositionInput,
    clients: WalletClients,
  ): PositionActionIntent;
}
