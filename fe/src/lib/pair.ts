/**
 * Split a pair label into its two symbols.
 *
 * Labels are written both ways across the app — "DAI/USDC" and "DAI / USDC" — so the separator is
 * matched with its surrounding space rather than assumed.
 */
export function pairSymbols(label: string): { base: string; quote: string } {
  const [base = "", quote = ""] = label.split("/").map((part) => part.trim());
  return { base, quote };
}
