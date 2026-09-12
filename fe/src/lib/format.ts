import { POOLS, TOKENS, type Pool } from "@/data";

export function price(sym: string): number {
  return TOKENS.find((t) => t.symbol === sym)?.price ?? 1;
}

const FIT_BANDS = [
  { upTo: 7, px: 76 },
  { upTo: 9, px: 66 },
  { upTo: 12, px: 54 },
] as const;

const OVERFLOW_PX = 44;

/** Amounts hold one size within each band, then keep shrinking beyond the widest band. */
export function fit(str: string | number): string {
  const n = Math.max(String(str).length, 1);
  const band = FIT_BANDS.find(({ upTo }) => n <= upTo);
  const cap = band?.px ?? OVERFLOW_PX;
  const widest = band?.upTo ?? n;
  return `min(${cap}px, ${(132 / widest).toFixed(1)}cqi, 8vh)`;
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
