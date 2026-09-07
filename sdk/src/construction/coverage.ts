export type CoverageState = "sufficient" | "over" | "empty";

export interface Coverage {
  state: CoverageState;
  /** Ship amount as a percentage of the wallet balance (0 when the balance is empty). */
  pctOfBalance: number;
}

/** How a token's ship amount sits against the maker's wallet balance — the wizard's
 * "uses 96% of balance · sufficient" inline state. */
export function coverage(shipAmount: bigint, walletBalance: bigint): Coverage {
  if (walletBalance === 0n) return { state: "empty", pctOfBalance: 0 };
  const pctOfBalance = Number((shipAmount * 10_000n) / walletBalance) / 100;
  return { state: shipAmount > walletBalance ? "over" : "sufficient", pctOfBalance };
}
