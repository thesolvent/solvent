import type { Asset, Quote } from "@/data";

/** A swap priced in display units; the adapter owns the base-unit conversion. */
export interface QuoteInput {
  from: Asset;
  to: Asset;
  amount: string;
}

/** Pricing a swap. Implemented by `adapters/http`. */
export interface SwapPort {
  quote(input: QuoteInput): Promise<Quote>;
}
