import type { QuoteResponse } from "@solvent/sdk/client";
import type { AggregateQuote } from "@solvent/sdk/cross-chain";
import { formatUnits } from "viem";

import { type Asset, type Quote } from "@/data";
import { DASH, percent, tokenAmount } from "@/lib/format";

/**
 * One priced route.
 *
 * The output is re-derived from raw base units rather than read off `display`, so the precision
 * the widget prints is its own choice and not whatever the server happened to send.
 */
export function toQuote(
  api: QuoteResponse,
  decimalsOut: number,
  input: Pick<Quote, "tokenIn" | "tokenOut" | "amountInRaw">,
): Quote {
  return {
    ...input,
    amountOut: tokenAmount(
      formatUnits(BigInt(api.amount_out.raw), decimalsOut),
    ),
    amountOutUsd: api.amount_out.usd ?? null,
    priceImpact: percent(api.price_impact_pct),
    makersSourced: api.makers_sourced,
    amountOutRaw: BigInt(api.amount_out.raw),
    expiresAt: Date.parse(api.expires_at),
  };
}

/** Present an aggregate cross-chain promise in the same slots as a same-chain quote. */
export function toCrossChainQuote(
  api: AggregateQuote,
  from: Asset,
  to: Asset,
  amountInRaw: bigint,
): Quote {
  const amountOutRaw = BigInt(api.amount_out);
  const amountOutUnits = formatUnits(amountOutRaw, to.decimals);
  const amountInUsd =
    from.price == null
      ? null
      : Number(formatUnits(amountInRaw, from.decimals)) * from.price;
  const amountOutUsd =
    to.price == null ? null : Number(amountOutUnits) * to.price;
  // The router knows what the route actually cost, so prefer its figure. The USD endpoints are a
  // fallback for a pair the valuation feed cannot price on both sides; unpriced either way there
  // is none to state, and stating zero would claim a free swap.
  const priceImpact =
    api.price_impact_bps != null
      ? percent(api.price_impact_bps / 100)
      : amountInUsd != null && amountOutUsd != null && amountInUsd > 0
        ? percent(Math.max(0, ((amountInUsd - amountOutUsd) / amountInUsd) * 100))
        : DASH;
  const makers = new Set(
    [...api.origin.sources, ...api.destination.sources].map((source) =>
      source.maker.toLowerCase(),
    ),
  );

  return {
    tokenIn: from.address,
    tokenOut: to.address,
    amountInRaw,
    amountOut: tokenAmount(amountOutUnits),
    amountOutUsd,
    priceImpact,
    makersSourced: makers.size,
    amountOutRaw,
    expiresAt: api.expires_at_unix * 1_000,
    crossChain: api,
  };
}
