import type { Pool as ApiPool } from "@solvent/sdk/client";

import { DASH, type Pool } from "@/data";

const USD = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  notation: "compact",
  maximumFractionDigits: 1,
});
const COUNT = new Intl.NumberFormat("en-US");

/** The Aqua deployment generation shown beside a fee tier. Fixed until the server reports one. */
const AQUA_VERSION = "v1";

type CurveMix = ApiPool["curve_mix"];

/** Wire counts per curve, in the words the filters use. A zero count means no maker prices that way. */
const CURVE_LABELS: [keyof CurveMix, string][] = [
  ["xyc", "Constant product"],
  ["concentrated", "Concentrated"],
  ["pegged", "Pegged"],
];

function curves(mix: CurveMix): string[] {
  return CURVE_LABELS.filter(([kind]) => mix[kind] > 0).map(
    ([, label]) => label,
  );
}

/** A signed move, arrowed the way it went. */
function change(pct?: number | null): string | undefined {
  if (pct == null) return undefined;
  return `${pct < 0 ? "↘" : "↗"} ${Math.abs(pct).toFixed(1)}%`;
}

function usd(value?: number | null): string {
  return value == null ? DASH : USD.format(value);
}

/** Basis points as the percentage the design labels a spread. */
function spread(min: number, max: number): string {
  const pct = (bps: number) => `${(bps / 100).toFixed(2)}%`;
  return min === max ? `${pct(min)} spread` : `${pct(min)}–${pct(max)} spread`;
}

/**
 * Translate a served pool into the display record the pool views render.
 *
 * The views hold pre-formatted strings rather than numbers, so every unit decision lives here:
 * the asset filters split `pair` on " / ", and an absent value must still read as text.
 */
export function toPool(api: ApiPool): Pool {
  return {
    pair: `${api.base.symbol} / ${api.quote.symbol}`,
    type: api.type,
    venue: `Aqua core · ${api.maker_count} ${api.maker_count === 1 ? "maker" : "makers"}`,
    range: spread(api.min_spread_bps, api.max_spread_bps),
    tvl: usd(api.tvl_usd),
    vol: usd(api.volume_24h_usd),
    fills: COUNT.format(api.fills_24h),
    fee: `${api.popular_fee_tier} · ${AQUA_VERSION}`,
    apr: api.apr_pct == null ? DASH : `${api.apr_pct.toFixed(1)}%`,
    feeTier: api.popular_fee_tier,
    curves: curves(api.curve_mix),
    ref: {
      base: api.base.address,
      quote: api.quote.address,
      baseDecimals: api.base.decimals,
      quoteDecimals: api.quote.decimals,
    },
    tvlUsd: api.tvl_usd,
    tvlChange: change(api.tvl_change_24h_pct),
    aprPct: api.apr_pct,
  };
}
