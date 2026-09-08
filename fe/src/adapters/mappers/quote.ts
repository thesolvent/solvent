import type { QuoteResponse } from "@solvent/sdk/client";
import { formatUnits } from "viem";

import type { Quote } from "@/data";

/** Small outputs need more places to say anything; large ones read as noise with them. */
function trimmed(amount: string): string {
  const value = Number(amount);
  const digits = value >= 1000 ? 2 : value >= 1 ? 4 : 6;
  return value.toLocaleString("en-US", {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
}

/**
 * One priced route.
 *
 * The output is re-derived from raw base units rather than read off `display`, so the precision
 * the widget prints is its own choice and not whatever the server happened to send.
 */
export function toQuote(api: QuoteResponse, decimalsOut: number): Quote {
  return {
    amountOut: trimmed(formatUnits(BigInt(api.amount_out.raw), decimalsOut)),
    amountOutUsd: api.amount_out.usd ?? 0,
    priceImpact: `${api.price_impact_pct.toFixed(2)}%`,
    makersSourced: api.makers_sourced,
    amountOutRaw: BigInt(api.amount_out.raw),
    expiresAt: Date.parse(api.expires_at),
  };
}
