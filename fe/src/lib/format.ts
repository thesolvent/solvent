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

const TOKEN_FRACTION_DIGITS = 6;

/** Token amounts are truncated rather than rounded so the UI never overstates a balance. */
export function formatTokenAmount(value: string): string {
  const match = /^([+-]?)(\d+)(?:\.(\d+))?$/.exec(value.trim());
  if (!match) return value;

  const [, sign, integer, fraction = ""] = match;
  const whole = integer
    .replace(/^0+(?=\d)/, "")
    .replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  const decimal = fraction.slice(0, TOKEN_FRACTION_DIGITS).replace(/0+$/, "");

  if (
    sign !== "-" &&
    /^0+$/.test(integer) &&
    !decimal &&
    /[1-9]/.test(fraction)
  )
    return "<0.000001";

  return `${sign}${whole}${decimal ? `.${decimal}` : ""}`;
}

export function poolByPair(pair: string): Pool | undefined {
  return POOLS.find((p) => p.pair === pair);
}
