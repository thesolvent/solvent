import type { Asset, Quote } from "@/data";

/** The unfiltered choice in each list; not a value any asset carries. */
export const ANY_TAG = "All";
export const ANY_NETWORK = "All networks";

function options(anyLabel: string, values: string[]): string[] {
  return [anyLabel, ...[...new Set(values)].sort()];
}

/**
 * The filter vocabularies, read off the assets themselves.
 *
 * A fixed list offers choices that match nothing — a tag no asset carries, or a chain this
 * deployment does not serve.
 */
export function tagOptions(assets: Asset[]): string[] {
  return options(
    ANY_TAG,
    assets.flatMap((asset) => asset.tags),
  );
}

export function networkOptions(assets: Asset[]): string[] {
  return options(
    ANY_NETWORK,
    assets.map((asset) => asset.net),
  );
}

/** Symbols quotable against `symbol`, read off the pairs it reports being part of. */
function counterparts(assets: Asset[], symbol: string): string[] {
  const asset = assets.find((candidate) => candidate.symbol === symbol);
  return (asset?.pairs ?? []).flatMap((pair) => {
    const legs = pair.split("/");
    return legs.includes(symbol) ? legs.filter((leg) => leg !== symbol) : [];
  });
}

/**
 * What the picker may offer for one leg.
 *
 * An asset in no pair leads nowhere, and the output leg is only ever an asset the input one is
 * actually quotable against — which also rules out picking the same asset twice.
 */
export function choices(
  assets: Asset[],
  leg: "from" | "to",
  from: string,
): Asset[] {
  if (leg === "from") return assets.filter((asset) => asset.pairs.length > 0);
  const allowed = new Set(counterparts(assets, from));
  return assets.filter((asset) => allowed.has(asset.symbol));
}

/** The first pair this deployment quotes at all. */
function firstPair(assets: Asset[]): Legs | null {
  for (const asset of assets) {
    const [base, quote] = (asset.pairs[0] ?? "").split("/");
    if (base && quote) return { fromToken: base, toToken: quote };
  }
  return null;
}

export interface Legs {
  fromToken: string;
  toToken: string;
}

/**
 * The legs moved onto a pair this deployment quotes, or `null` when they already name one.
 *
 * The mock opens on a pair no server need serve, and changing the input leg can strand the output
 * one — both leave the widget asking for a price that cannot exist.
 */
export function settleLegs(
  assets: Asset[],
  fromToken: string,
  toToken: string,
): Legs | null {
  if (!assets.length) return null;
  const allowed = counterparts(assets, fromToken);
  if (allowed.includes(toToken)) return null;
  return allowed.length
    ? { fromToken, toToken: allowed[0] }
    : firstPair(assets);
}

/** What the action button says, and whether there is anything to press it for. */
export interface SwapAction {
  label: string;
  ready: boolean;
}

/**
 * The button is also where a trade says why it cannot happen, so a problem is never silent.
 *
 * The server's own words carry the reason, so a clearer message upstream needs no change here.
 */
export function swapAction(input: {
  connected: boolean;
  /** The network to move to before signing, or `undefined` when already on it. */
  switchTo: string | undefined;
  submitting: boolean;
  submitted: boolean;
  amount: number;
  pricing: boolean;
  quote: Quote | undefined;
  problem: string | undefined;
  submissionProblem?: string;
}): SwapAction {
  const {
    connected,
    switchTo,
    submitting,
    submitted,
    amount,
    pricing,
    quote,
    problem,
    submissionProblem,
  } = input;
  if (submitting) return { label: "Confirm in your wallet", ready: false };
  if (submitted) return { label: "Intent submitted to Aqua", ready: false };
  // A trade that cannot happen says so whether or not a wallet is attached.
  if (problem) return { label: problem, ready: false };
  if (amount <= 0) return { label: "Enter an amount", ready: false };
  if (!connected) return { label: "Connect a wallet", ready: true };
  // An order names its chain, and a wallet will not sign for one it is not on.
  if (switchTo) return { label: `Switch to ${switchTo}`, ready: true };
  if (pricing || !quote) {
    return { label: "Finding the best price", ready: false };
  }
  return {
    label: submissionProblem ? `${submissionProblem} — try again` : "Swap",
    ready: true,
  };
}

/** EIP-1193's code for a request the person declined. */
const USER_REJECTED = 4001;

/** Guard against a cause chain that loops back on itself. */
const MAX_CAUSES = 10;

/**
 * Whether the person simply said no.
 *
 * Matched on name and code rather than `instanceof`: the wallet stack resolves several copies of
 * viem, and a class from one copy is not the same object as the class from another.
 */
function isWalletRejection(error: unknown): boolean {
  let cause = error;
  for (let depth = 0; cause && depth < MAX_CAUSES; depth += 1) {
    const { name, code } = cause as { name?: string; code?: number };
    if (name === "UserRejectedRequestError" || code === USER_REJECTED) {
      return true;
    }
    cause = (cause as { cause?: unknown }).cause;
  }
  return false;
}

/**
 * Why a submission failed, in words meant for the person who tried.
 *
 * The server writes its refusals for a reader, but a wallet writes them for a developer — dumping
 * one on the button gives a stack trace where a sentence belongs.
 */
export function submissionProblem(error: Error | null): string | undefined {
  if (!error) return undefined;
  if (isSwapDeclined(error)) return "The resolver declined this swap";
  if (error.name === "InputValidationError") return error.message;
  return isWalletRejection(error)
    ? "Wallet request rejected"
    : "Could not submit the swap";
}

export function isSwapDeclined(error: Error | null): boolean {
  return error?.name === "SwapDeclinedError";
}
