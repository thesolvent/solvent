import type {
  PositionCreationStatus,
  PositionSubmissionOptions,
  PositionTransactionStatus,
  PositionTransactionSubmissionOptions,
} from "@solvent/sdk/positions";
import type { WalletClients } from "@solvent/sdk/swap";

export type {
  PositionCreationStatus,
  PositionSubmissionOptions,
  PositionTransactionStatus,
  PositionTransactionSubmissionOptions,
};

export interface CreateToken {
  address: `0x${string}`;
  decimals: number;
  symbol: string;
  name: string;
  logoUri?: string | null;
  net?: string;
  chainLogoUri?: string | null;
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
  submit(options?: PositionSubmissionOptions): Promise<CreatedPosition>;
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
  submit(
    options?: PositionTransactionSubmissionOptions,
  ): Promise<PositionActionResult>;
}

export interface PositionsPort {
  pairs(wallet?: string): Promise<CreatePair[]>;
  history(
    pair: CreatePair,
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
