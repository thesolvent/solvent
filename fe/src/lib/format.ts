import { POOLS, TOKENS, type Pool } from "@/data";

export function price(sym: string): number {
  return TOKENS.find((t) => t.symbol === sym)?.price ?? 1;
}

/**
 * Amount displays shrink as digits are added so long values never overflow their
 * cell — capped px, then container-relative, then viewport-relative.
 */
/** How wide a figure may be before it is printed a step smaller, and the size for that step. */
const FIT_BANDS = [
  { upTo: 7, px: 76 },
  { upTo: 9, px: 66 },
  { upTo: 12, px: 54 },
] as const;

const OVERFLOW_PX = 44;

/**
 * The size a figure is printed at — capped px, then container-relative, then viewport-relative.
 *
 * Stepped, not continuous. The px cap was already banded, but the container-relative term was
 * computed from the exact character count, and `min` takes whichever is smaller — so at most
 * container widths the twitchy term won and the number re-scaled on every keystroke. Each band now
 * sizes for its widest member, so the type only changes when a figure crosses into the next band.
 * Past the last band a figure has to keep shrinking or it cannot fit at all.
 */
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
