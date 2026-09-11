import type {
  SwapSubmissionOptions,
  SwapSubmissionStatus,
  WalletClients,
} from "@solvent/sdk/swap";
import type { Asset, Quote, SubmittedSwap } from "@/data";

export type { SwapSubmissionOptions, SwapSubmissionStatus };

export interface QuoteInput {
  from: Asset;
  to: Asset;
  amount: string;
}

export interface SwapInput extends QuoteInput {
  quote: Quote;
  swapper: string;
  slippagePct: number;
}

/** One payment authorization, retained by the caller when retrying submission. */
export interface SwapIntent {
  submit(options?: SwapSubmissionOptions): Promise<SubmittedSwap>;
}

export interface SwapPort {
  quote(input: QuoteInput): Promise<Quote>;
  createIntent(input: SwapInput, clients: WalletClients): SwapIntent;
}
