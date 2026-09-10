import { skipToken, useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { parseTokenAmount } from "@solvent/sdk/validation";
import type { Asset, Quote } from "@/data";
import { useServices } from "./context";

/** Long enough that typing an amount does not price every keystroke. */
const SETTLE_MS = 300;

/** Refresh this far ahead of the server's own expiry, so what is on screen is never past it. */
const REFRESH_MARGIN_MS = 5_000;

/** Never poll faster than this, however short a life the server gives a quote. */
const MIN_REFRESH_MS = 5_000;

/** Retry cadence for a quote that did not come back, so an outage heals without retyping. */
const RETRY_MS = 10_000;

/** Hold a value still until it stops changing. */
function useSettled<T>(value: T, ms: number): T {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setSettled(value), ms);
    return () => clearTimeout(timer);
  }, [value, ms]);
  return settled;
}

/** A price is good until the server says it is not, so refresh just before that — and keep asking
 *  on a cadence when there is no price at all, so a failure heals on its own. */
function refreshIn(quote: Quote | undefined): number {
  if (!quote) return RETRY_MS;
  const remaining = quote.expiresAt - Date.now() - REFRESH_MARGIN_MS;
  // An unreadable expiry must not leave the price frozen on screen.
  return Number.isFinite(remaining)
    ? Math.max(MIN_REFRESH_MS, remaining)
    : RETRY_MS;
}

/** The server says why a trade cannot be priced; pass that on rather than inventing a phrase. */
function reason(error: Error | null): string | undefined {
  if (!error) return undefined;
  const said = error.message.trim();
  if (!said) return "Could not price this trade";
  return said.charAt(0).toUpperCase() + said.slice(1);
}

export interface QuoteState {
  quote: Quote | undefined;
  pricing: boolean;
  /** Why no price could be had, in the server's own words. */
  problem: string | undefined;
}

/** Reprice settled inputs and refresh before expiry; hide the old quote as soon as inputs change. */
export function useQuote(
  from: Asset | undefined,
  to: Asset | undefined,
  amount: string,
): QuoteState {
  const { swap } = useServices();
  const settled = useSettled(amount, SETTLE_MS);
  let inputProblem: string | undefined;
  if (from && settled !== "") {
    try {
      parseTokenAmount(settled, from.decimals, "Swap amount");
    } catch (error) {
      inputProblem =
        error instanceof Error ? error.message : "Enter a valid swap amount";
    }
  }
  const quotable =
    from !== undefined &&
    to !== undefined &&
    (from.chainId !== to.chainId ||
      from.address.toLowerCase() !== to.address.toLowerCase()) &&
    inputProblem === undefined &&
    settled !== "";

  const { data, isFetching, error, failureReason } = useQuery({
    queryKey: [
      "quote",
      from?.chainId,
      from?.address,
      to?.chainId,
      to?.address,
      settled,
    ],
    queryFn: quotable
      ? () => swap.quote({ from, to, amount: settled })
      : skipToken,
    staleTime: 0,
    refetchInterval: ({ state }) => refreshIn(state.data),
    refetchOnWindowFocus: true,
  });

  return {
    quote: amount === settled && !error && !failureReason ? data : undefined,
    pricing: amount !== settled || (quotable && isFetching),
    // `error` only lands once retries are spent, and never while they are paused; the
    // reason is known from the first failure, and a trade that cannot happen should say so.
    problem:
      amount === settled
        ? (inputProblem ?? reason(error ?? failureReason))
        : undefined,
  };
}
