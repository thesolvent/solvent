import type { WalletClients } from "@solvent/sdk/swap";
import type { Asset, Quote, SubmittedSwap } from "@/data";

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
  submit(): Promise<SubmittedSwap>;
}

export interface SwapPort {
  quote(input: QuoteInput): Promise<Quote>;
  createIntent(input: SwapInput, clients: WalletClients): SwapIntent;
}
