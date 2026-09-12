import { formatUnits } from "viem";
import { parseTokenAmount } from "@solvent/sdk/validation";

import type { Asset, Quote } from "@/data";
import { chain } from "@/adapters/wallet/config";
import type { SwapSubmissionStatus } from "@/ports/swap";
import { SolventApiError, SolventNetworkError } from "@solvent/sdk/client";
import { CrossChainApiError } from "@solvent/sdk/cross-chain";
import { InputValidationError } from "@solvent/sdk/validation";
import { tokenAmount } from "./format";

/** The unfiltered choice in each list; not a value any asset carries. */
export const ANY_TAG = "All";
export const ANY_NETWORK = "All networks";

/**
 * Whether a keystroke leaves something that is still on its way to being a number.
 *
 * `inputMode` only hints at which keyboard to raise; it refuses nothing, so a typed letter
 * reaches the amount and every consumer downstream has to survive it. A partial entry — "", "0.",
 * "." — is accepted because it is a decimal mid-typing, not a wrong one.
 */
export function isAmountDraft(value: string): boolean {
  return /^\d*\.?\d*$/.test(value);
}

/** Chain-qualified identity keeps the same token symbol on two networks selectable. */
export function assetKey(asset: Asset): string {
  return `${asset.chainId}:${asset.address.toLowerCase()}`;
}

/** Accept legacy symbol state while moving explicit picker choices to chain-qualified keys. */
export function selectedAsset(
  assets: Asset[],
  selection: string,
): Asset | undefined {
  return assets.find(
    (asset) => assetKey(asset) === selection || asset.symbol === selection,
  );
}

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

/**
 * The chains a leg can actually use, read off the choices already offered rather than off the
 * catalog: the destination is constrained to what the source can route to, so a chain the list
 * cannot reach must not be offered as a filter. `All` appears only when there is a choice to make.
 */
export function chainOptions(offered: Asset[]): string[] {
  const nets = options(
    ANY_NETWORK,
    offered.map((asset) => asset.net),
  );
  return nets.length > 2 ? nets : nets.slice(1);
}

/**
 * The chain a cross-chain swap starts from.
 *
 * The deployment routes one way — the settler lives on the origin chain and the app on the
 * destination, both holding the other's id immutably — so the origin is the build's own chain
 * rather than whichever asset happened to sort first in a list concatenated from two deployments.
 */
export function crossChainOrigin(assets: Asset[]): number | undefined {
  return assets.some((asset) => asset.chainId === chain.id)
    ? chain.id
    : assets[0]?.chainId;
}

/** A chain's mark, taken from any asset that reports living on it. */
export function chainLogo(assets: Asset[], net: string): string | undefined {
  return assets.find((asset) => asset.net === net)?.chainLogoUri ?? undefined;
}

/** Symbols quotable against `symbol`, read off the pairs it reports being part of. */
function counterparts(asset: Asset | undefined): string[] {
  const symbol = asset?.symbol ?? "";
  return (asset?.pairs ?? []).flatMap((pair) => {
    const legs = pair.split("/");
    return legs.includes(symbol) ? legs.filter((leg) => leg !== symbol) : [];
  });
}

function remoteRepresentation(assets: Asset[], source: Asset | undefined) {
  return assets.find(
    (asset) =>
      asset.chainId !== source?.chainId && asset.symbol === source?.symbol,
  );
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
  crossChain = false,
): Asset[] {
  if (leg === "from") {
    if (!crossChain) return assets.filter((asset) => asset.pairs.length > 0);
    return assets.filter(
      (asset) =>
        asset.chainId === crossChainOrigin(assets) &&
        remoteRepresentation(assets, asset)?.pairs.length,
    );
  }
  const source = selectedAsset(assets, from);
  const quoteSource = crossChain
    ? remoteRepresentation(assets, source)
    : source;
  const allowed = new Set(counterparts(quoteSource));
  const sourceNetwork = source?.net;
  return assets.filter(
    (asset) =>
      allowed.has(asset.symbol) &&
      (crossChain
        ? asset.chainId === quoteSource?.chainId
        : !sourceNetwork || asset.net === sourceNetwork),
  );
}

/** The first asset this deployment can quote from. */
function firstSource(assets: Asset[]): string | undefined {
  return assets.find((asset) => asset.pairs.length > 0)?.symbol;
}

function firstCrossChainSource(assets: Asset[]): string | undefined {
  return assets.find(
    (asset) =>
      asset.chainId === crossChainOrigin(assets) &&
      remoteRepresentation(assets, asset)?.pairs.length,
  )?.symbol;
}

export interface Legs {
  fromToken: string;
  toToken: string;
}

/**
 * The source moved onto an asset this deployment quotes, with an incompatible output cleared.
 *
 * A destination is a user's choice. Loading the catalog may select a valid source, but it must not
 * silently choose the other side of the trade.
 */
export function settleLegs(
  assets: Asset[],
  fromToken: string,
  toToken: string,
  crossChain = false,
): Legs | null {
  if (!assets.length) return null;
  const source = selectedAsset(assets, fromToken);
  const quoteSource = crossChain
    ? remoteRepresentation(assets, source)
    : source;
  const settledSource =
    source && quoteSource?.pairs.length
      ? fromToken
      : crossChain
        ? firstCrossChainSource(assets)
        : firstSource(assets);
  if (!settledSource) return null;
  if (settledSource !== fromToken)
    return { fromToken: settledSource, toToken: "" };
  if (!toToken) return null;
  const settledAsset = selectedAsset(assets, settledSource);
  const settledQuoteAsset = crossChain
    ? remoteRepresentation(assets, settledAsset)
    : settledAsset;
  const destination = selectedAsset(assets, toToken);
  const compatible = counterparts(settledQuoteAsset).includes(
    destination?.symbol ?? "",
  );
  const sourceNetwork = settledAsset?.net;
  const destinationNetwork = destination?.net;
  const sameNetwork =
    !sourceNetwork ||
    !destinationNetwork ||
    sourceNetwork === destinationNetwork;
  const expectedDestinationChain = crossChain
    ? settledQuoteAsset?.chainId
    : destination?.chainId;
  if (
    compatible &&
    destination?.chainId === expectedDestinationChain &&
    (crossChain || sameNetwork)
  )
    return null;
  return { fromToken: settledSource, toToken: "" };
}

const BPS = 10_000n;

/**
 * The output floor an order names.
 *
 * The widget and the order call this from one place, so the amount shown as protected is the
 * amount the order actually enforces. A slippage outside the representable range is clamped
 * rather than printed as a nonsense floor; submission rejects it separately.
 */
export function minimumOutput(
  amountOutRaw: bigint,
  slippagePct: number,
): bigint {
  const requested = Number.isFinite(slippagePct)
    ? BigInt(Math.round(slippagePct * 100))
    : BPS;
  const tolerance = requested < 0n ? 0n : requested > BPS ? BPS : requested;
  return (amountOutRaw * (BPS - tolerance)) / BPS;
}

/** The floor as the widget prints it, in the output token's units. */
export function minimumReceived(
  quote: Quote,
  decimalsOut: number,
  slippagePct: number,
): string {
  return tokenAmount(
    formatUnits(minimumOutput(quote.amountOutRaw, slippagePct), decimalsOut),
  );
}

/** Whether the wallet is short of what the trade would spend. Unknown holdings block nothing. */
export function isAboveBalance(
  amount: string,
  decimals: number,
  balance: bigint | undefined,
): boolean {
  if (balance === undefined) return false;
  try {
    return parseTokenAmount(amount, decimals, "Swap amount") > balance;
  } catch {
    // An unparseable amount is already reported as a pricing problem.
    return false;
  }
}

/** How far the trade moves the price against itself, in bands that change what the button does. */
export type ImpactLevel = "normal" | "high" | "severe";

const HIGH_IMPACT_PCT = 1;
const SEVERE_IMPACT_PCT = 5;

export function impactLevel(priceImpact: string | undefined): ImpactLevel {
  const pct = Number.parseFloat(priceImpact ?? "");
  if (!Number.isFinite(pct) || pct < HIGH_IMPACT_PCT) return "normal";
  return pct < SEVERE_IMPACT_PCT ? "high" : "severe";
}

/** What the action button says, and whether there is anything to press it for. */
export interface SwapAction {
  label: string;
  ready: boolean;
  retry?: true;
}

export function swapSubmissionLabel(
  status: SwapSubmissionStatus | undefined,
  inputToken: string | undefined,
): string {
  switch (status?.kind) {
    case "approving":
      return `Approve ${inputToken ?? "token"}…`;
    case "signing":
      return "Sign swap…";
    case "submitting":
      return "Submitting swap…";
    case "confirming":
      return "Confirming swap…";
    case "preparing":
    default:
      return "Preparing swap…";
  }
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
  /** The symbol the wallet is short of, or `undefined` when it can cover the trade. */
  short?: string;
  impact?: ImpactLevel;
  impactAcknowledged?: boolean;
  submissionStatus?: SwapSubmissionStatus;
  inputToken?: string;
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
    short,
    impact = "normal",
    impactAcknowledged = false,
    submissionStatus,
    inputToken,
  } = input;
  if (submitting) {
    return {
      label: swapSubmissionLabel(submissionStatus, inputToken),
      ready: false,
    };
  }
  if (submitted) return { label: "Intent submitted to Aqua", ready: false };
  // A trade that cannot happen says so whether or not a wallet is attached.
  if (problem) return { label: problem, ready: false };
  if (submissionProblem) {
    return {
      label: `${submissionProblem} — try again`,
      ready: true,
      retry: true,
    };
  }
  if (amount <= 0) return { label: "Enter an amount", ready: false };
  if (!connected) return { label: "Connect a wallet", ready: true };
  // Signing a trade the wallet cannot pay for fails deep in the wallet, with no reason given here.
  if (short) return { label: `Insufficient ${short} balance`, ready: false };
  // An order names its chain, and a wallet will not sign for one it is not on.
  if (switchTo) return { label: `Switch to ${switchTo}`, ready: true };
  if (pricing || !quote) {
    return { label: "Finding the best price", ready: false };
  }
  // A trade this far out of line is worth a second press rather than one stray click.
  if (impact === "severe" && !impactAcknowledged)
    return { label: "Confirm price impact", ready: true };
  return { label: "Swap", ready: true };
}

/** EIP-1193's code for a request the person declined. */
const USER_REJECTED = 4001;

/** Guard against a cause chain that loops back on itself. */
const MAX_CAUSES = 10;
const SAFE_WALLET_MESSAGES = new Set([
  "Wallet account changed",
  "Wrong wallet network",
  "Wrong RPC network",
  "Token amount must be positive",
  "Insufficient token balance",
  "Approval cannot be below the required amount",
  "Token refused approval",
  "Token allowance was not updated",
  "Approval reverted",
  "Transaction reverted",
]);

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
export function submissionProblem(
  error: Error | null,
  fallback = "Could not submit the swap",
): string | undefined {
  if (!error) return undefined;
  if (isSwapDeclined(error)) return "The resolver declined this swap";
  if (
    error instanceof InputValidationError ||
    error instanceof CrossChainApiError ||
    error instanceof SolventApiError ||
    error instanceof SolventNetworkError ||
    error.name === "InputValidationError" ||
    error.name === "CrossChainApiError" ||
    error.name === "SolventApiError" ||
    error.name === "SolventNetworkError" ||
    SAFE_WALLET_MESSAGES.has(error.message)
  )
    return error.message;
  return isWalletRejection(error) ? "Wallet request rejected" : fallback;
}

export function isSwapDeclined(error: Error | null): boolean {
  return error?.name === "SwapDeclinedError";
}
