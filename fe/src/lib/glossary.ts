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
  feeTier: "Share of each trade paid to the makers.",

  // ── Pool detail ────────────────────────────────────────────────────────────
  virtual: "Size the maker committed to quote.",
  actual: "What the maker's wallet and allowance can cover right now.",
  priceImpact: "How far your own size moves the price against you.",

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
} as const;

export type GlossaryKey = keyof typeof GLOSSARY;
