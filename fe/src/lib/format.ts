import { POOLS, TOKENS, type Pool } from "@/data";

export function price(sym: string): number {
  return TOKENS.find((t) => t.symbol === sym)?.price ?? 1;
}

/**
 * Amount displays shrink as digits are added so long values never overflow their
 * cell — capped px, then container-relative, then viewport-relative.
 */
export function fit(str: string | number): string {
  const n = String(str).length;
  const cap = n <= 7 ? 76 : n <= 9 ? 66 : n <= 12 ? 54 : 44;
  return `min(${cap}px, ${(132 / Math.max(n, 1)).toFixed(1)}cqi, 8vh)`;
}

/** Small outputs need more places to say anything; large ones read as noise with them. */
export function trimmedAmount(amount: string): string {
  const value = Number(amount);
  const digits = value >= 1000 ? 2 : value >= 1 ? 4 : 6;
  return value.toLocaleString("en-US", {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
}

export function money(n: number): string {
  if (n >= 1e9) {
    return n.toLocaleString("en-US", {
      notation: "compact",
      maximumFractionDigits: 2,
    });
  }
  return n.toLocaleString("en-US", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  });
}

export function poolByPair(pair: string): Pool | undefined {
  return POOLS.find((p) => p.pair === pair);
}
