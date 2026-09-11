/**
 * What each term on screen means, in one line.
 *
 * One place so a term reads identically wherever it appears, and so the wording can be reviewed
 * as a set rather than found in eleven views.
 *
 * House rules for an entry: define the term, and add a consequence only when it changes a
 * decision. No analogies, no reassurance, no restating the label. State what a number is measured
 * over, never how to feel about it. Claim nothing the served data does not carry.
 */
export const GLOSSARY = {
  // ── Pools ──────────────────────────────────────────────────────────────────
  netApr:
    "24h fee income over pool value, annualised. A quiet day moves it sharply.",
  tvl: "USD value of everything makers have committed to this pool.",
  spread: "Gap between the pool's buy and sell price, before gas.",
  feeTier: "Share of each trade paid to the makers.",
  recommended: "Highest net APR among pools that report one.",
  poolStable: "Both sides are stablecoins.",
  poolVolatile: "At least one side floats.",

  // ── Curve shapes ───────────────────────────────────────────────────────────
  curveXyc:
    "Quotes across the whole price range. Always active, thinner at any one price.",
  curveConcentrated:
    "Quotes only inside a price band. Deeper in range, inactive outside it.",
  curvePegged: "Quotes around a fixed price, for assets meant to trade at par.",
  curveMixed: "This pool's makers do not all price on the same shape.",

  // ── Pool detail ────────────────────────────────────────────────────────────
  virtual: "Size the maker committed to quote.",
  actual: "What the maker's wallet and allowance can cover right now.",
  bestPrice: "Price for a trade small enough not to move the curve.",
  priceImpact: "How far your own size moves the price against you.",
  totalLiquidity:
    "Fillable across every maker before impact passes the selected tier.",
  aggregatedDepth: "Every maker's quotable size on this pair, added together.",
  marketPrice:
    "Reference price from the market feed. Not this pool's own quote.",
  zeroInventory:
    "Fills are sourced from makers' own wallets; the resolver holds nothing.",

  // ── Swap ───────────────────────────────────────────────────────────────────
  fills: "Makers sourced to fill this swap.",
  maxSlippage:
    "If the price moves further than this before the fill lands, the swap does not execute.",

  // ── Maker ──────────────────────────────────────────────────────────────────
  sharedLiquidity:
    "USD committed across this maker's positions, held in their own wallet.",
  walletBalance: "USD this maker holds, whether committed or not.",
  pullable:
    "Most Aqua can move from this wallet: the lesser of balance and allowance.",
  sharedLiqRatio:
    "Pullable over committed. Under 100% means some commitments cannot be met in full.",
  activePositions: "Positions currently quoting.",
  fillShare: "Share of this pair's fills sourced from this maker.",
  fillLatency: "Median time from submission to confirmation.",
  rangeFull: "Quotes at any price.",
  rangeBounded: "Quotes only inside a set band.",
  coverage: "Pullable balance as a share of what the position committed.",

  // ── Explorer: stats ────────────────────────────────────────────────────────
  blockHeight: "Latest block the indexer has read.",
  events24h: "Aqua events seen in the last 24 hours.",
  tradesSettled: "Trades confirmed on chain in the last 24 hours.",
  medianImpact: "Middle price impact across recent trades.",
  activeMakers: "Makers with at least one position quoting.",

  // ── Explorer: trade status ─────────────────────────────────────────────────
  statusQuoted: "Priced, not yet committed to.",
  statusReserved: "Maker balance held against this trade.",
  statusSimulated: "Passed a dry run against the chain.",
  statusSubmitted: "Sent to the mempool, not yet confirmed.",
  statusConfirmed: "Settled on chain.",
  statusDeclined: "Not worth filling at the price offered.",
  statusFailed: "Reached the chain and reverted.",

  // ── Explorer: Aqua events ──────────────────────────────────────────────────
  eventRegister: "A maker created a position.",
  eventPush: "A maker added committed balance.",
  eventPull: "A maker withdrew committed balance.",
  eventDock: "A maker retired a position; it no longer quotes.",

  // ── Trade detail ───────────────────────────────────────────────────────────
  taker: "The address that signed the order.",
  resolver: "The filler that sourced and settled it.",
  orderHash: "The order's identifier, as the reactor sees it.",
  deadline: "After this, the order can no longer be filled.",
  minimumReceived:
    "Least you can receive and still have the swap execute, after slippage.",

  // ── Create position ────────────────────────────────────────────────────────
  immutable:
    "A position cannot be changed after it is created. Withdraw and create another to change the range or fee.",
} as const;

export type GlossaryKey = keyof typeof GLOSSARY;

/** Trade statuses as the API spells them, mapped to their definition. */
export const STATUS_TERMS: Record<string, GlossaryKey> = {
  quoted: "statusQuoted",
  reserved: "statusReserved",
  simulated: "statusSimulated",
  submitted: "statusSubmitted",
  confirmed: "statusConfirmed",
  declined: "statusDeclined",
  failed: "statusFailed",
};

/** Aqua event types as the activity feed spells them. */
export const EVENT_TERMS: Record<string, GlossaryKey> = {
  register: "eventRegister",
  push: "eventPush",
  pull: "eventPull",
  dock: "eventDock",
};
