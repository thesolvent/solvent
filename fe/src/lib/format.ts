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

/** Numeric-input sanitizer: digits and a single decimal point, max 16 chars. */
export function clean(v: string): string {
  let out = String(v).replace(/[^0-9.]/g, "");
  const first = out.indexOf(".");
  if (first > -1) {
    out = out.slice(0, first + 1) + out.slice(first + 1).replace(/\./g, "");
  }
  return out.slice(0, 16);
}

export function poolByPair(pair: string): Pool | undefined {
  return POOLS.find((p) => p.pair === pair);
}
